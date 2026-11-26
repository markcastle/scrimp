//! Phase 0 probe for Scrimp.
//!
//! Compresses real files on real test volumes and writes down exactly what
//! Windows did, as a Markdown report. It answers questions S1 to S5 in
//! TASKS.md. It is deliberately throwaway: it records every result and
//! never stops at the first error, because the errors are the findings.
//!
//! ```text
//! wof-probe <report.md> --main N:\probe [--volume LABEL=ROOT]... [--smb \\localhost\share]
//! ```

mod win;

use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use win::*;
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, WOF_PROVIDER_FILE};
use windows::Win32::System::IO::{CancelIoEx, CancelSynchronousIo, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentThreadId, OpenThread, THREAD_TERMINATE, WaitForSingleObject,
};

const MB: usize = 1024 * 1024;
const WOF: [&str; 4] = ["XPRESS4K", "XPRESS8K", "XPRESS16K", "LZX"];
const RW: u32 = GENERIC_READ | GENERIC_WRITE;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(report_path) = args.first().cloned() else {
        eprintln!("usage: wof-probe <report.md> --main DIR [--volume LABEL=ROOT]... [--smb UNC]");
        std::process::exit(2);
    };
    let mut main_dir = None;
    let mut volumes = Vec::new();
    let mut smb = None;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--main" => main_dir = it.next().map(PathBuf::from),
            "--volume" => {
                if let Some((label, root)) = it.next().and_then(|v| v.split_once('=')) {
                    volumes.push((label.to_string(), PathBuf::from(root)));
                }
            }
            "--smb" => smb = it.next().map(PathBuf::from),
            other => eprintln!("ignoring {other}"),
        }
    }
    let Some(main_dir) = main_dir else {
        eprintln!("--main is required");
        std::process::exit(2);
    };
    let _ = fs::create_dir_all(&main_dir);

    let mut r = String::new();
    let started = Instant::now();
    let _ = writeln!(r, "# Scrimp Phase 0 probe report\n");
    let _ = writeln!(r, "Windows: `{}`\n", run_text("cmd", &["/c", "ver"]).trim());
    let _ = writeln!(r, "Main test folder: `{}`\n", main_dir.display());

    section(&mut r, "S0: does WOF work on this volume at all?", || {
        s0_diagnose(&main_dir)
    });
    section(
        &mut r,
        "S1a: compress with each method, through wofutil.dll and through FSCTL",
        || s1_methods(&main_dir),
    );
    section(&mut r, "S1b: which access rights each operation needs", || {
        s1_access(&main_dir)
    });
    section(&mut r, "S1c: read-only files", || s1_read_only(&main_dir));
    section(&mut r, "S1d: files another program has open", || s1_sharing(&main_dir));
    section(&mut r, "S1e: writing to a compressed file", || s1_writes(&main_dir));
    section(&mut r, "S1f: changing from one method to another directly", || {
        s1_transitions(&main_dir)
    });
    section(
        &mut r,
        "S1g: hardlinks, alternate data streams and sparse files",
        || s1_special(&main_dir),
    );
    section(&mut r, "S2: which size is the real size on disk", || {
        s2_sizes(&main_dir)
    });
    section(&mut r, "S3: files that won't compress", || s3_not_beneficial(&main_dir));
    section(&mut r, "S4: cancelling compression of a large file", || {
        s4_cancel(&main_dir)
    });
    section(&mut r, "S5: other volumes, encryption and network shares", || {
        s5_volumes(&main_dir, &volumes, smb.as_deref())
    });
    section(&mut r, "Extra: Windows' own compressed system files", s_extra_system);

    let _ = writeln!(r, "\nProbe finished in {:.0} s.", started.elapsed().as_secs_f64());
    if let Err(e) = fs::write(&report_path, &r) {
        eprintln!("couldn't write {report_path}: {e}");
        std::process::exit(1);
    }
    println!("{r}");
}

fn section(r: &mut String, title: &str, body: impl FnOnce() -> String) {
    let t = Instant::now();
    eprintln!("== {title}");
    let text = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body))
        .unwrap_or_else(|_| "**This section panicked; see the job log.**\n".into());
    let _ = writeln!(r, "## {title}\n\n{text}\n_Took {:.1} s._\n", t.elapsed().as_secs_f64());
}

// ---------------------------------------------------------------- helpers

fn text_data(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 128);
    let mut i = 0u64;
    while out.len() < len {
        let _ = write!(
            out,
            "Row {i}: Example.dat moved to D:\\Archive on day {} with status {}.\n",
            i % 365,
            i % 7
        );
        i += 1;
    }
    out.truncate(len);
    out
}

fn random_data(len: usize) -> Vec<u8> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn fresh(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    let _ = set_attributes(&p, 0x80);
    let _ = fs::remove_file(&p);
    if let Err(e) = fs::write(&p, data) {
        eprintln!("couldn't write {}: {e}", p.display());
    }
    p
}

fn hash(path: &Path) -> Option<blake3::Hash> {
    let mut f = fs::File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 4 * MB];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize())
}

fn same(path: &Path, expected: &Option<blake3::Hash>) -> &'static str {
    match (hash(path), expected) {
        (Some(a), Some(b)) if a == *b => "identical",
        (None, _) => "unreadable",
        _ => "**DIFFERENT**",
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Times {
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
}

fn times(path: &Path) -> Times {
    let m = fs::metadata(path).ok();
    Times {
        created: m.as_ref().and_then(|m| m.created().ok()),
        modified: m.as_ref().and_then(|m| m.modified().ok()),
        accessed: m.as_ref().and_then(|m| m.accessed().ok()),
    }
}

fn compare_times(before: Times, after: Times) -> String {
    let mut changed = Vec::new();
    if before.created != after.created {
        changed.push("created");
    }
    if before.modified != after.modified {
        changed.push("modified");
    }
    if before.accessed != after.accessed {
        changed.push("accessed");
    }
    if changed.is_empty() {
        "unchanged".into()
    } else {
        format!("**changed: {}**", changed.join(", "))
    }
}

fn size_text(r: WinResult<u64>) -> String {
    match r {
        Ok(n) => n.to_string(),
        Err(e) => format!("({e})"),
    }
}

fn run_text(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        })
        .unwrap_or_else(|e| format!("couldn't run {program}: {e}"))
}

/// What compact.exe says about one file: the line naming it.
fn compact_says(path: &Path) -> String {
    let out = run_text("compact", &[&path.to_string_lossy()]);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    out.lines()
        .find(|l| l.contains(&name))
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "(no line)".into())
        .replace('|', "/")
}

fn cell(s: impl AsRef<str>) -> String {
    s.as_ref().replace('|', "/").replace('\n', " ")
}

/// Applies a state by name: "none", a WOF algorithm, or "LZNT1".
fn apply(path: &Path, to: &str) -> WinResult<()> {
    let h = open(path, RW, 0)?;
    match to {
        "none" => {
            let wof = wof_state(path)
                .map(|(e, p, _)| e && p == WOF_PROVIDER_FILE)
                .unwrap_or(false);
            if wof { wof_remove(&h) } else { ntfs_set(&h, 0) }
        }
        "LZNT1" => ntfs_set(&h, 2),
        alg => wof_set_api(&h, wof_id(alg), 0),
    }
}

// ---------------------------------------------------------------- S0

fn s0_diagnose(dir: &Path) -> String {
    let data = text_data(8 * MB);
    let mut out = String::new();

    let p = fresh(dir, "s0-compact.txt", &data);
    let said = run_text("compact", &["/c", "/exe:lzx", &p.to_string_lossy()]);
    let _ = writeln!(
        out,
        "**compact.exe /c /exe:lzx** on 8 MiB of text. State after: {}. GetCompressedFileSize: {}. It said:\n\n```text\n{}\n```\n",
        describe_state(&p),
        size_text(compressed_size(&p)),
        said.trim()
    );

    let p = fresh(dir, "s0-version.txt", &data);
    let version = open(&p, GENERIC_READ, 0).and_then(|h| wof_driver_version(&h, WOF_PROVIDER_FILE));
    let wim = open(&p, GENERIC_READ, 0).and_then(|h| wof_driver_version(&h, 1));
    let _ = writeln!(
        out,
        "**WofGetDriverVersion** on a file here: file provider {}, WIM provider {}.\n",
        version.map_or_else(|e| e.to_string(), |v| format!("0x{v:X}")),
        wim.map_or_else(|e| e.to_string(), |v| format!("0x{v:X}"))
    );

    out.push_str("**Our call, opened different ways** (LZX):\n\n| Open | Access | wofutil | FSCTL |\n| --- | --- | --- | --- |\n");
    let ways: [(&str, bool, &str, u32); 4] = [
        ("backup semantics", true, "GENERIC_READ + GENERIC_WRITE", RW),
        ("plain", false, "GENERIC_READ + GENERIC_WRITE", RW),
        ("plain", false, "GENERIC_READ", GENERIC_READ),
        ("plain", false, "READ_DATA", READ_DATA | SYNCHRONIZE),
    ];
    for (how, backup, access_name, access) in ways {
        let mut cells = Vec::new();
        for route in ["wofutil", "FSCTL"] {
            let p = fresh(dir, "s0-call.txt", &data);
            let h = if backup {
                open(&p, access, 0)
            } else {
                open_plain(&p, access)
            };
            let r = h.and_then(|h| {
                if route == "wofutil" {
                    wof_set_api(&h, wof_id("LZX"), 0)
                } else {
                    wof_set_fsctl(&h, wof_id("LZX"))
                }
            });
            cells.push(format!("{} ({})", show(&r), describe_state(&p)));
        }
        let _ = writeln!(
            out,
            "| {how} | {access_name} | {} |",
            cells.iter().map(cell).collect::<Vec<_>>().join(" | ")
        );
    }
    let _ = writeln!(
        out,
        "\n`fltmc instances -f wof`:\n\n```text\n{}\n```",
        run_text("fltmc", &["instances", "-f", "wof"]).trim()
    );
    out
}

// ---------------------------------------------------------------- S1

fn s1_methods(dir: &Path) -> String {
    let data = text_data(8 * MB);
    let mut out = String::from(
        "8 MiB of text, handle opened with GENERIC_READ | GENERIC_WRITE.\n\n\
         | Method | Route | Result | State after | Logical | GetCompressedFileSize | Timestamps | Content | compact.exe says |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for alg in WOF.iter().map(|a| (*a, true)).chain([("LZNT1", false)]) {
        let routes: &[&str] = if alg.1 {
            &["wofutil", "FSCTL"]
        } else {
            &["FSCTL_SET_COMPRESSION"]
        };
        for route in routes {
            let p = fresh(dir, &format!("s1a-{}-{route}.txt", alg.0), &data);
            let expected = hash(&p);
            let before = times(&p);
            std::thread::sleep(Duration::from_millis(50));
            let result = open(&p, RW, 0).and_then(|h| match *route {
                "wofutil" => wof_set_api(&h, wof_id(alg.0), 0),
                "FSCTL" => wof_set_fsctl(&h, wof_id(alg.0)),
                _ => ntfs_set(&h, 2),
            });
            let _ = writeln!(
                out,
                "| {} | {route} | {} | {} | {} | {} | {} | {} | {} |",
                alg.0,
                cell(show(&result)),
                cell(describe_state(&p)),
                data.len(),
                size_text(compressed_size(&p)),
                compare_times(before, times(&p)),
                same(&p, &expected),
                compact_says(&p)
            );
        }
    }
    out
}

fn s1_access(dir: &Path) -> String {
    let masks: [(&str, u32); 7] = [
        ("GENERIC_READ", GENERIC_READ),
        ("READ_DATA", READ_DATA | SYNCHRONIZE),
        (
            "READ_DATA + WRITE_ATTRIBUTES",
            READ_DATA | WRITE_ATTRIBUTES | SYNCHRONIZE,
        ),
        (
            "READ_ATTRIBUTES + WRITE_ATTRIBUTES",
            READ_ATTRIBUTES | WRITE_ATTRIBUTES | SYNCHRONIZE,
        ),
        ("READ_DATA + WRITE_DATA", READ_DATA | WRITE_DATA | SYNCHRONIZE),
        ("GENERIC_READ + WRITE_ATTRIBUTES", GENERIC_READ | WRITE_ATTRIBUTES),
        ("GENERIC_READ + GENERIC_WRITE", RW),
    ];
    let data = text_data(MB);
    let mut out = String::from(
        "1 MiB of text. Each cell is a fresh file opened with only that access.\n\n\
         | Access | WOF LZX (wofutil) | WOF LZX (FSCTL) | Remove WOF | NTFS LZNT1 on | NTFS off |\n\
         | --- | --- | --- | --- | --- | --- |\n",
    );
    for (name, mask) in masks {
        let mut cells = Vec::new();
        let p = fresh(dir, "s1b.txt", &data);
        cells.push(show(&open(&p, mask, 0).and_then(|h| wof_set_api(&h, wof_id("LZX"), 0))));
        let p = fresh(dir, "s1b.txt", &data);
        cells.push(show(&open(&p, mask, 0).and_then(|h| wof_set_fsctl(&h, wof_id("LZX")))));
        let p = fresh(dir, "s1b.txt", &data);
        let _ = apply(&p, "LZX");
        cells.push(show(&open(&p, mask, 0).and_then(|h| wof_remove(&h))));
        let p = fresh(dir, "s1b.txt", &data);
        cells.push(show(&open(&p, mask, 0).and_then(|h| ntfs_set(&h, 2))));
        let p = fresh(dir, "s1b.txt", &data);
        let _ = apply(&p, "LZNT1");
        cells.push(show(&open(&p, mask, 0).and_then(|h| ntfs_set(&h, 0))));
        let _ = writeln!(
            out,
            "| {name} | {} |",
            cells.iter().map(cell).collect::<Vec<_>>().join(" | ")
        );
    }
    out
}

fn s1_read_only(dir: &Path) -> String {
    let data = text_data(MB);
    let mut out = String::from(
        "| Attempt | Result | State after | Still read-only | Content |\n| --- | --- | --- | --- | --- |\n",
    );
    let attempts: [(&str, u32, &str); 4] = [
        ("WOF LZX, GENERIC_READ", GENERIC_READ, "LZX"),
        ("WOF LZX, GENERIC_READ + GENERIC_WRITE", RW, "LZX"),
        ("NTFS LZNT1, GENERIC_READ + GENERIC_WRITE", RW, "LZNT1"),
        (
            "NTFS LZNT1, READ_DATA + WRITE_ATTRIBUTES",
            READ_DATA | WRITE_ATTRIBUTES | SYNCHRONIZE,
            "LZNT1",
        ),
    ];
    for (name, mask, what) in attempts {
        let p = fresh(dir, "s1c.txt", &data);
        let expected = hash(&p);
        let _ = set_attributes(&p, 0x1);
        let result = open(&p, mask, 0).and_then(|h| {
            if what == "LZNT1" {
                ntfs_set(&h, 2)
            } else {
                wof_set_api(&h, wof_id(what), 0)
            }
        });
        let ro = fs::metadata(&p).map(|m| m.permissions().readonly()).unwrap_or(false);
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {ro} | {} |",
            cell(show(&result)),
            cell(describe_state(&p)),
            same(&p, &expected)
        );
        let _ = set_attributes(&p, 0x80);
    }
    out
}

fn s1_sharing(dir: &Path) -> String {
    let data = text_data(MB);
    let mut out = String::from(
        "Another handle is held open while Scrimp's handle (GENERIC_READ | GENERIC_WRITE, sharing everything) compresses with LZX.\n\n\
         | Other program has it open for | Its share mode | Open | WOF LZX | State after |\n| --- | --- | --- | --- | --- |\n",
    );
    let cases: [(&str, u32, &str, u32); 4] = [
        ("reading", GENERIC_READ, "read", 0x1),
        ("reading", GENERIC_READ, "read + write", 0x3),
        ("writing", RW, "read + write", 0x3),
        ("reading", GENERIC_READ, "none", 0x0),
    ];
    for (what, access, share_name, share) in cases {
        let p = fresh(dir, "s1d.txt", &data);
        let other = open_shared(&p, access, share);
        let ours = open(&p, RW, 0);
        let result = ours
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|h| wof_set_api(h, wof_id("LZX"), 0));
        let _ = writeln!(
            out,
            "| {what} ({}) | {share_name} | {} | {} | {} |",
            if other.is_ok() { "held" } else { "couldn't open" },
            cell(show(&ours)),
            cell(show(&result)),
            cell(describe_state(&p))
        );
        drop(ours);
        drop(other);
    }
    out
}

fn s1_writes(dir: &Path) -> String {
    let data = text_data(4 * MB);
    let mut out = String::from(
        "4 MiB of text, compressed, then one line appended by an ordinary write.\n\n\
         | Compressed with | Flags | Compress result | State before write | Write | State after write | Size on disk after | Content |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    let cases: [(&str, u32); 5] = [("LZX", 0), ("XPRESS4K", 0), ("LZX", 1), ("XPRESS8K", 1), ("LZNT1", 0)];
    for (alg, flags) in cases {
        let p = fresh(dir, "s1e.txt", &data);
        let result = open(&p, RW, 0).and_then(|h| {
            if alg == "LZNT1" {
                ntfs_set(&h, 2)
            } else {
                wof_set_api(&h, wof_id(alg), flags)
            }
        });
        let state_before = describe_state(&p);
        let extra = b"Row extra: appended after compression.\n";
        let write = fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .and_then(|mut f| f.write_all(extra));
        let mut expected_data = data.clone();
        expected_data.extend_from_slice(extra);
        let content = match fs::read(&p) {
            Ok(d) if d == expected_data => "as written",
            Ok(_) => "**DIFFERENT**",
            Err(_) => "unreadable",
        };
        let _ = writeln!(
            out,
            "| {alg} | {} | {} | {} | {} | {} | {} | {content} |",
            if flags == 1 { "COMPRESS_ON_WRITE" } else { "none" },
            cell(show(&result)),
            cell(state_before),
            write.map_or_else(|e| e.to_string(), |_| "ok".into()),
            cell(describe_state(&p)),
            size_text(compressed_size(&p))
        );
    }
    out
}

fn s1_transitions(dir: &Path) -> String {
    let states = ["none", "XPRESS4K", "XPRESS8K", "XPRESS16K", "LZX", "LZNT1"];
    let data = text_data(4 * MB);
    let mut out = String::from(
        "4 MiB of text. Put in the first state, then asked straight for the second, without removing anything first.\n\n\
         | From | To | Setting up | Change | State after | Content |\n| --- | --- | --- | --- | --- | --- |\n",
    );
    for from in states {
        for to in states {
            if from == to {
                continue;
            }
            let p = fresh(dir, "s1f.txt", &data);
            let expected = hash(&p);
            let setup = if from == "none" { Ok(()) } else { apply(&p, from) };
            let change = apply(&p, to);
            let _ = writeln!(
                out,
                "| {from} | {to} | {} | {} | {} | {} |",
                cell(show(&setup)),
                cell(show(&change)),
                cell(describe_state(&p)),
                same(&p, &expected)
            );
        }
    }
    out
}

fn s1_special(dir: &Path) -> String {
    let data = text_data(4 * MB);
    let mut out = String::new();

    let p = fresh(dir, "s1g-link-a.txt", &data);
    let link = dir.join("s1g-link-b.txt");
    let _ = fs::remove_file(&link);
    let made = hard_link(&link, &p);
    let result = apply(&link, "LZX");
    let _ = writeln!(
        out,
        "**Hardlink.** Made: {}. Compressed the second name with LZX: {}. First name is now: {}. Second: {}.\n",
        show(&made),
        show(&result),
        describe_state(&p),
        describe_state(&link)
    );

    let p = fresh(dir, "s1g-ads.txt", &data);
    let stream = PathBuf::from(format!("{}:extra", p.display()));
    let side = text_data(MB / 2);
    let wrote = fs::write(&stream, &side);
    let expected = hash(&p);
    let result = apply(&p, "LZX");
    let stream_back = fs::read(&stream).map(|d| d == side);
    let _ = writeln!(
        out,
        "**Alternate data stream.** Wrote stream: {:?}. Compressed with LZX: {}. State: {}. Main content: {}. Stream intact: {:?}. Streams now: {:?}.\n",
        wrote.is_ok(),
        show(&result),
        describe_state(&p),
        same(&p, &expected),
        stream_back.ok(),
        streams(&p).unwrap_or_default()
    );

    let p = fresh(dir, "s1g-sparse.txt", &data);
    let sparse = open(&p, RW, 0).and_then(|h| set_sparse(&h));
    let result = apply(&p, "LZX");
    let ntfs = fresh(dir, "s1g-sparse-ntfs.txt", &data);
    let _ = open(&ntfs, RW, 0).and_then(|h| set_sparse(&h));
    let ntfs_result = apply(&ntfs, "LZNT1");
    let _ = writeln!(
        out,
        "**Sparse file.** Marked sparse: {}. WOF LZX: {} (state {}). NTFS LZNT1 on another sparse file: {} (state {}).\n",
        show(&sparse),
        show(&result),
        describe_state(&p),
        show(&ntfs_result),
        describe_state(&ntfs)
    );
    out
}

// ---------------------------------------------------------------- S2

fn s2_sizes(dir: &Path) -> String {
    let data = text_data(64 * MB);
    let mut out = String::from(
        "64 MiB of text per row. \"Free space used\" is the drop in the volume's free space, the ground truth.\n\n\
         | Method | Logical | GetCompressedFileSize | AllocationSize | Free space used | Streams (name: size) |\n\
         | --- | --- | --- | --- | --- | --- |\n",
    );
    for alg in ["none", "XPRESS4K", "XPRESS8K", "XPRESS16K", "LZX", "LZNT1"] {
        let p = dir.join(format!("s2-{alg}.txt"));
        let _ = fs::remove_file(&p);
        let free_before = free_space(dir);
        let _ = fs::write(&p, &data);
        let result = if alg == "none" { Ok(()) } else { apply(&p, alg) };
        std::thread::sleep(Duration::from_millis(500));
        let free_after = free_space(dir);
        let used = match (free_before, free_after) {
            (Ok(b), Ok(a)) => (b as i128 - a as i128).to_string(),
            _ => "?".into(),
        };
        let streams = streams(&p)
            .map(|v| {
                v.iter()
                    .map(|(n, s)| format!("{n}: {s}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|e| e.to_string());
        let _ = writeln!(
            out,
            "| {alg}{} | {} | {} | {} | {used} | {} |",
            if result.is_ok() { "" } else { " (failed)" },
            data.len(),
            size_text(compressed_size(&p)),
            size_text(allocation_size(&p)),
            cell(streams)
        );
    }
    out
}

// ---------------------------------------------------------------- S3

fn s3_not_beneficial(dir: &Path) -> String {
    let mut out = String::from(
        "| File | Method | Route | Result | State after | GetCompressedFileSize | Content |\n| --- | --- | --- | --- | --- | --- | --- |\n",
    );
    let files: [(&str, Vec<u8>); 5] = [
        ("8 MiB random", random_data(8 * MB)),
        ("100 bytes of text", text_data(100)),
        ("empty", Vec::new()),
        ("4,096 bytes of text", text_data(4096)),
        ("8 MiB half random", {
            let mut d = text_data(4 * MB);
            d.extend(random_data(4 * MB));
            d
        }),
    ];
    for (label, data) in files {
        for (alg, route) in [
            ("LZX", "wofutil"),
            ("LZX", "FSCTL"),
            ("XPRESS4K", "wofutil"),
            ("LZNT1", "FSCTL_SET_COMPRESSION"),
        ] {
            let p = fresh(dir, "s3.bin", &data);
            let expected = hash(&p);
            let result = open(&p, RW, 0).and_then(|h| match route {
                "wofutil" => wof_set_api(&h, wof_id(alg), 0),
                "FSCTL" => wof_set_fsctl(&h, wof_id(alg)),
                _ => ntfs_set(&h, 2),
            });
            let _ = writeln!(
                out,
                "| {label} | {alg} | {route} | {} | {} | {} | {} |",
                cell(show(&result)),
                cell(describe_state(&p)),
                size_text(compressed_size(&p)),
                same(&p, &expected)
            );
        }
    }
    out
}

// ---------------------------------------------------------------- S4

fn write_big(path: &Path, len: usize) {
    let chunk = text_data(16 * MB);
    let _ = fs::remove_file(path);
    if let Ok(mut f) = fs::File::create(path) {
        let mut written = 0;
        while written < len {
            let n = chunk.len().min(len - written);
            if f.write_all(&chunk[..n]).is_err() {
                break;
            }
            written += n;
        }
    }
}

fn s4_cancel(dir: &Path) -> String {
    let mut out = String::new();

    // How fast is LZX here, uncancelled?
    let p = dir.join("s4-baseline.txt");
    write_big(&p, 256 * MB);
    let t = Instant::now();
    let result = apply(&p, "LZX");
    let secs = t.elapsed().as_secs_f64();
    let _ = writeln!(
        out,
        "**Baseline.** 256 MiB of text with LZX through wofutil, uncancelled: {} in {secs:.1} s ({:.0} MiB/s). State: {}.\n",
        show(&result),
        256.0 / secs.max(0.001),
        describe_state(&p)
    );
    let _ = fs::remove_file(&p);

    // Overlapped FSCTL, then CancelIoEx after two seconds.
    let p = dir.join("s4-overlapped.txt");
    write_big(&p, 1024 * MB);
    let expected = hash(&p);
    let line = (|| -> WinResult<String> {
        let h = open(&p, RW, FILE_FLAG_OVERLAPPED.0)?;
        // SAFETY: plain event creation.
        let event = unsafe { CreateEventW(None, true, false, None) }?;
        let input = Box::new(ExternalBacking::new(wof_id("LZX")));
        let mut ov = Box::new(OVERLAPPED { hEvent: event, ..Default::default() });
        let t = Instant::now();
        // SAFETY: `input` and `ov` are boxed and outlive the operation: we
        // wait for completion below before dropping them.
        let started = unsafe { wof_set_fsctl_overlapped(&h, &input, &mut *ov) };
        let started_text = show(&started);
        let pending = matches!(&started, Err(e) if e.win32() == Some(997));
        let returned_after = t.elapsed().as_secs_f64();
        let mut cancel_text = "not attempted: the call had already finished".to_string();
        let mut outcome = started_text.clone();
        if pending {
            std::thread::sleep(Duration::from_secs(2));
            // SAFETY: `ov` is the live OVERLAPPED of the pending operation.
            cancel_text = show(&unsafe { CancelIoEx(h.0, Some(&*ov)) }.map_err(WinError::from));
            let mut n = 0u32;
            // SAFETY: waits for the operation `ov` describes.
            outcome = show(&unsafe { GetOverlappedResult(h.0, &*ov, &mut n, true) }.map_err(WinError::from));
        }
        let total = t.elapsed().as_secs_f64();
        // SAFETY: the operation is complete, so the event can be closed.
        unsafe {
            let _ = CloseHandle(event);
        }
        drop(h);
        Ok(format!(
            "DeviceIoControl returned after {returned_after:.2} s with: {started_text}. Pending: {pending}. CancelIoEx after 2 s: {cancel_text}. \
             Final result: {outcome}, {total:.1} s after starting. State: {}. Content: {}.",
            describe_state(&p),
            same(&p, &expected)
        ))
    })()
    .unwrap_or_else(|e| format!("couldn't run: {e}"));
    let _ = writeln!(out, "**1 GiB, LZX, overlapped FSCTL, CancelIoEx.** {line}\n");
    let _ = fs::remove_file(&p);

    // Synchronous wofutil in a thread, then CancelSynchronousIo.
    let p = dir.join("s4-sync.txt");
    write_big(&p, 1024 * MB);
    let expected = hash(&p);
    let (tx, rx) = std::sync::mpsc::channel();
    let path = p.clone();
    let worker = std::thread::spawn(move || {
        // SAFETY: opens a handle to this very thread for CancelSynchronousIo.
        let me = unsafe { OpenThread(THREAD_TERMINATE, false, GetCurrentThreadId()) }.map(|h| h.0 as usize);
        let _ = tx.send(me);
        let t = Instant::now();
        let r = apply(&path, "LZX");
        (r, t.elapsed().as_secs_f64())
    });
    let thread_handle = rx.recv().ok().and_then(Result::ok);
    std::thread::sleep(Duration::from_secs(2));
    let cancel = match thread_handle {
        Some(raw) => {
            let h = windows::Win32::Foundation::HANDLE(raw as *mut _);
            // SAFETY: `h` is the worker thread's handle, still running or just finished.
            let r = show(&unsafe { CancelSynchronousIo(h) }.map_err(WinError::from));
            // SAFETY: closes the thread handle opened above.
            unsafe {
                let _ = CloseHandle(h);
            }
            r
        }
        None => "no thread handle".into(),
    };
    let (result, secs) = worker.join().unwrap_or((
        Err(WinError {
            hresult: -1,
            message: "thread panicked".into(),
        }),
        0.0,
    ));
    let _ = writeln!(
        out,
        "**1 GiB, LZX, wofutil on a thread, CancelSynchronousIo after 2 s.** Cancel: {cancel}. Compression returned: {} after {secs:.1} s. State: {}. Content: {}.\n",
        show(&result),
        describe_state(&p),
        same(&p, &expected)
    );
    let _ = fs::remove_file(&p);

    let _ = WaitForSingleObject;
    let _ = WAIT_OBJECT_0;
    out
}

// ---------------------------------------------------------------- S5

fn try_everything(dir: &Path, label: &str) -> String {
    let data = text_data(MB);
    let _ = fs::create_dir_all(dir);
    let p = fresh(dir, "s5.txt", &data);
    if !p.exists() {
        return format!("| {label} | couldn't create a file in {} | | | | |\n", dir.display());
    }
    let expected = hash(&p);
    let wof = apply(&p, "LZX");
    let wof_state = describe_state(&p);
    let remove = if wof.is_ok() {
        show(&apply(&p, "none"))
    } else {
        "n/a".into()
    };
    let ntfs = apply(&p, "LZNT1");
    let ntfs_state = describe_state(&p);
    let _ = apply(&p, "none");
    format!(
        "| {label} | {} ({}) | {} | {} ({}) | {} |\n",
        cell(show(&wof)),
        cell(wof_state),
        cell(remove),
        cell(show(&ntfs)),
        cell(ntfs_state),
        same(&p, &expected)
    )
}

fn s5_volumes(main: &Path, volumes: &[(String, PathBuf)], smb: Option<&Path>) -> String {
    let mut out = String::from(
        "| Volume | Filesystem | Cluster size | Reports compression support |\n| --- | --- | --- | --- |\n",
    );
    for (label, root) in volumes {
        match volume(root) {
            Ok((fs_name, flags, cluster)) => {
                let _ = writeln!(out, "| {label} | {fs_name} | {cluster} | {} |", flags & 0x10 != 0);
            }
            Err(e) => {
                let _ = writeln!(out, "| {label} | ({e}) | | |");
            }
        }
    }
    out.push_str(
        "\n| Where | WOF LZX (state) | Remove WOF | NTFS LZNT1 (state) | Content |\n| --- | --- | --- | --- | --- |\n",
    );
    for (label, root) in volumes {
        out.push_str(&try_everything(&root.join("probe"), label));
    }
    if let Some(share) = smb {
        out.push_str(&try_everything(share, &format!("SMB share {}", share.display())));
    }

    // EFS: encrypt first, then try to compress.
    let p = fresh(main, "s5-efs.txt", &text_data(MB));
    let enc = encrypt(&p);
    let wof = apply(&p, "LZX");
    let ntfs = apply(&p, "LZNT1");
    let _ = writeln!(
        out,
        "\n**EFS.** Encrypt: {}. Then WOF LZX: {}. NTFS LZNT1: {}. State: {}.\n",
        show(&enc),
        show(&wof),
        show(&ntfs),
        describe_state(&p)
    );
    out
}

fn s_extra_system() -> String {
    let mut out = String::from("| File | State | GetCompressedFileSize | Logical |\n| --- | --- | --- | --- |\n");
    for f in [
        r"C:\Windows\System32\notepad.exe",
        r"C:\Windows\System32\ntdll.dll",
        r"C:\Windows\System32\kernel32.dll",
        r"C:\Windows\explorer.exe",
        r"C:\Windows\System32\drivers\etc\hosts",
    ] {
        let p = Path::new(f);
        let _ = writeln!(
            out,
            "| `{f}` | {} | {} | {} |",
            cell(describe_state(p)),
            size_text(compressed_size(p)),
            fs::metadata(p).map(|m| m.len().to_string()).unwrap_or_default()
        );
    }
    let _ = writeln!(
        out,
        "\n`compact /compactos:query`:\n\n```text\n{}\n```",
        run_text("compact", &["/compactos:query"]).trim()
    );
    out
}
