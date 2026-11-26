//! `cargo xtask imports <file>...`: proves a built program needs nothing
//! but Windows.
//!
//! Scrimp promises it runs on a fresh Windows 10 or 11 with nothing else
//! installed: no Visual C++ redistributable, no .NET. That holds because
//! `.cargo/config.toml` links the C runtime statically (`+crt-static`). If
//! that setting were lost, the programs would quietly start needing
//! `vcruntime140.dll`, and work on every developer's PC but not on a clean
//! one. This reads each program's import tables and refuses any DLL that
//! isn't part of Windows.

use anyhow::{Context, Result, bail};

/// Windows' own DLLs that Scrimp may use. Anything else fails, so adding a
/// DLL here is a choice someone made, not an accident. Lower case.
const WINDOWS_DLLS: &[&str] = &[
    "advapi32.dll",
    "bcrypt.dll",
    "bcryptprimitives.dll",
    // COM's own runtime, which the Explorer menu and the dialogs use.
    "combase.dll",
    "comctl32.dll",
    "crypt32.dll",
    "kernel32.dll",
    "ntdll.dll",
    "ole32.dll",
    "oleaut32.dll",
    "secur32.dll",
    "shell32.dll",
    "shlwapi.dll",
    "user32.dll",
    "userenv.dll",
    "wofutil.dll",
    "ws2_32.dll",
];

/// Whether a DLL is part of Windows. The `api-ms-win-core-*` names are
/// Windows' own API sets; `api-ms-win-crt-*` are the C runtime's, which
/// mean the runtime was linked dynamically, so they don't count.
pub fn is_windows(dll: &str) -> bool {
    let dll = dll.to_ascii_lowercase();
    WINDOWS_DLLS.contains(&dll.as_str()) || (dll.starts_with("api-ms-win-") && !dll.starts_with("api-ms-win-crt-"))
}

fn u16_at(data: &[u8], at: usize) -> Result<u16> {
    let bytes = data.get(at..at + 2).context("The file ends too soon")?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    let bytes = data.get(at..at + 4).context("The file ends too soon")?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Every DLL a 64-bit Windows program or DLL imports, including those it
/// loads on first use (delay-loaded).
pub fn imported_dlls(data: &[u8]) -> Result<Vec<String>> {
    if data.get(0..2) != Some(b"MZ") {
        bail!("Not a Windows program");
    }
    let pe = u32_at(data, 0x3c)? as usize;
    if data.get(pe..pe + 4) != Some(b"PE\0\0") {
        bail!("Not a Windows program");
    }
    let coff = pe + 4;
    let sections = usize::from(u16_at(data, coff + 2)?);
    let optional = coff + 20;
    let optional_size = usize::from(u16_at(data, coff + 16)?);
    if u16_at(data, optional)? != 0x20b {
        bail!("Not a 64-bit Windows program");
    }
    // Data directories start 112 bytes into a 64-bit optional header.
    let directory = |index: usize| -> Result<u32> { u32_at(data, optional + 112 + index * 8) };

    let table = optional + optional_size;
    let to_offset = |rva: u32| -> Result<usize> {
        for s in 0..sections {
            let at = table + s * 40;
            let (size, address, raw) = (u32_at(data, at + 8)?, u32_at(data, at + 12)?, u32_at(data, at + 20)?);
            if rva >= address && rva < address + size.max(u32_at(data, at + 16)?) {
                return Ok((rva - address + raw) as usize);
            }
        }
        bail!("An import points outside the file")
    };
    let name_at = |rva: u32| -> Result<String> {
        let start = to_offset(rva)?;
        let end = data[start..]
            .iter()
            .position(|&b| b == 0)
            .context("A DLL name has no end")?;
        Ok(String::from_utf8_lossy(&data[start..start + end]).into_owned())
    };

    let mut dlls = Vec::new();
    // Ordinary imports: 20-byte descriptors with the name at 12, ending in zeros.
    // Delay-loaded imports: 32-byte descriptors with the name at 4.
    for (index, size, name_offset) in [(1, 20, 12), (13, 32, 4)] {
        let rva = directory(index)?;
        if rva == 0 {
            continue;
        }
        let mut at = to_offset(rva)?;
        loop {
            let name = u32_at(data, at + name_offset)?;
            if name == 0 {
                break;
            }
            dlls.push(name_at(name)?);
            at += size;
        }
    }
    dlls.sort_by_key(|d| d.to_ascii_lowercase());
    dlls.dedup_by_key(|d| d.to_ascii_lowercase());
    Ok(dlls)
}

/// Checks each file, returning a line about each, or an error naming every
/// DLL that isn't Windows'.
pub fn check(files: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    let mut report = Vec::new();
    let mut strangers = Vec::new();
    for (name, data) in files {
        let dlls = imported_dlls(data).with_context(|| format!("Couldn't read {name}"))?;
        let outside: Vec<&String> = dlls.iter().filter(|d| !is_windows(d)).collect();
        if outside.is_empty() {
            report.push(format!("{name}: only Windows ({})", dlls.join(", ")));
        } else {
            strangers.push(format!(
                "{name} needs {}, which isn't part of Windows. If it's the C runtime, check `+crt-static` in .cargo/config.toml; if it's a Windows DLL, add it to WINDOWS_DLLS in crates/xtask/src/imports.rs.",
                outside.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    if !strangers.is_empty() {
        bail!("{}", strangers.join("\n"));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal 64-bit program with one section holding its import tables.
    fn program(imports: &[&str], delayed: &[&str]) -> Vec<u8> {
        const SECTION_RVA: u32 = 0x1000;
        const RAW: usize = 0x200;
        let mut data = vec![0u8; RAW];
        data[0..2].copy_from_slice(b"MZ");
        let pe = 0x40usize;
        data[0x3c..0x40].copy_from_slice(&(pe as u32).to_le_bytes());
        data[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        data[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        let optional_size = 240u16;
        data[coff + 16..coff + 18].copy_from_slice(&optional_size.to_le_bytes());
        let optional = coff + 20;
        data[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());

        // The section: descriptors first, then the names.
        let mut section = vec![0u8; 0x400];
        let names_at = 0x200usize;
        let mut name_cursor = names_at;
        let mut place = |name: &str, section: &mut Vec<u8>| -> u32 {
            let at = name_cursor;
            section[at..at + name.len()].copy_from_slice(name.as_bytes());
            name_cursor += name.len() + 1;
            SECTION_RVA + at as u32
        };
        for (i, dll) in imports.iter().enumerate() {
            let rva = place(dll, &mut section);
            let at = i * 20 + 12;
            section[at..at + 4].copy_from_slice(&rva.to_le_bytes());
        }
        let delay_at = 0x100usize;
        for (i, dll) in delayed.iter().enumerate() {
            let rva = place(dll, &mut section);
            let at = delay_at + i * 32 + 4;
            section[at..at + 4].copy_from_slice(&rva.to_le_bytes());
        }
        let dir = |index: usize, rva: u32, data: &mut Vec<u8>| {
            let at = optional + 112 + index * 8;
            data[at..at + 4].copy_from_slice(&rva.to_le_bytes());
        };
        dir(1, SECTION_RVA, &mut data);
        if !delayed.is_empty() {
            dir(13, SECTION_RVA + delay_at as u32, &mut data);
        }
        let table = optional + usize::from(optional_size);
        let put = |at: usize, v: u32, data: &mut Vec<u8>| data[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(table + 8, section.len() as u32, &mut data);
        put(table + 12, SECTION_RVA, &mut data);
        put(table + 16, section.len() as u32, &mut data);
        put(table + 20, RAW as u32, &mut data);
        data.extend(section);
        data
    }

    #[test]
    fn imports_and_delayed_imports_are_read() {
        let exe = program(&["KERNEL32.dll", "ntdll.dll", "kernel32.dll"], &["comctl32.dll"]);
        assert_eq!(
            imported_dlls(&exe).unwrap(),
            ["comctl32.dll", "KERNEL32.dll", "ntdll.dll"]
        );
    }

    #[test]
    fn only_windows_passes() {
        let fine = program(
            &["KERNEL32.dll", "api-ms-win-core-synch-l1-2-0.dll", "wofutil.dll"],
            &[],
        );
        assert!(check(&[("scrimp.exe".into(), fine)]).is_ok());

        let runtime = program(
            &["KERNEL32.dll", "VCRUNTIME140.dll", "api-ms-win-crt-heap-l1-1-0.dll"],
            &[],
        );
        let err = check(&[("scrimp.exe".into(), runtime)]).unwrap_err().to_string();
        assert!(
            err.contains("scrimp.exe needs api-ms-win-crt-heap-l1-1-0.dll, VCRUNTIME140.dll"),
            "{err}"
        );
        assert!(err.contains("+crt-static"), "{err}");
    }

    #[test]
    fn what_isnt_a_program_says_so() {
        assert!(imported_dlls(b"hello").is_err());
        assert!(imported_dlls(b"MZ").is_err());
        let mut exe = program(&["KERNEL32.dll"], &[]);
        exe[0x40 + 24] = 0x0b;
        exe[0x40 + 25] = 0x01;
        assert!(imported_dlls(&exe).unwrap_err().to_string().contains("64-bit"));
    }
}
