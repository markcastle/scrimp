//! `cargo xtask winget <version> <SHA256SUMS> <folder>`: the winget manifest
//! for a release, so `winget install MarkCastle.Scrimp` can install it.
//!
//! winget installs from a manifest in Microsoft's public repository,
//! `microsoft/winget-pkgs`: three small YAML files saying what the package
//! is, where its installer is and how to run it quietly. The Release
//! workflow writes them from the release's own `SHA256SUMS`, so the checksum
//! in the manifest is the one people's downloads are already checked
//! against, and attaches them to the release. Submitting them is a pull
//! request to winget-pkgs, made by a person: see docs/developing.md.
//!
//! The installer is NSIS, per user, so winget runs it with `/S` and, when
//! someone picks a folder, `/D=`; both are what `installer/scrimp.nsi`
//! already supports. The product code is the name of its Installed apps
//! entry, which is how winget recognises Scrimp is already installed.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub const IDENTIFIER: &str = "MarkCastle.Scrimp";
const PUBLISHER: &str = "Mark Castle";
const REPO: &str = "https://github.com/markcastle/scrimp";
const INSTALLER: &str = "Scrimp-windows-x64-setup.exe";
/// The key `installer/scrimp.nsi` writes under Uninstall.
const PRODUCT_CODE: &str = "Scrimp";
const MANIFEST_VERSION: &str = "1.6.0";
const SCHEMA: &str = "https://aka.ms/winget-manifest";

fn header(kind: &str) -> String {
    format!("# yaml-language-server: $schema={SCHEMA}.{kind}.{MANIFEST_VERSION}.schema.json\n")
}

/// The three files, as (file name, contents).
pub fn manifests(version: &str, installer_sha256: &str, release_date: &str) -> Vec<(String, String)> {
    let sha = installer_sha256.to_ascii_uppercase();
    let version_file = format!(
        "{}PackageIdentifier: {IDENTIFIER}\nPackageVersion: {version}\nDefaultLocale: en-US\nManifestType: version\nManifestVersion: {MANIFEST_VERSION}\n",
        header("version")
    );
    let installer_file = format!(
        "{}PackageIdentifier: {IDENTIFIER}
PackageVersion: {version}
InstallerType: nullsoft
Scope: user
InstallModes:
  - interactive
  - silent
UpgradeBehavior: install
Commands:
  - scrimp
ReleaseDate: {release_date}
AppsAndFeaturesEntries:
  - DisplayName: Scrimp
    Publisher: {PUBLISHER}
    DisplayVersion: {version}
    ProductCode: {PRODUCT_CODE}
Installers:
  - Architecture: x64
    InstallerUrl: {REPO}/releases/download/v{version}/{INSTALLER}
    InstallerSha256: {sha}
    ProductCode: {PRODUCT_CODE}
ManifestType: installer
ManifestVersion: {MANIFEST_VERSION}
",
        header("installer")
    );
    let locale_file = format!(
        "{}PackageIdentifier: {IDENTIFIER}
PackageVersion: {version}
PackageLocale: en-US
Publisher: {PUBLISHER}
PublisherUrl: https://github.com/markcastle
PublisherSupportUrl: {REPO}/issues
PackageName: Scrimp
PackageUrl: {REPO}
License: MIT
LicenseUrl: {REPO}/blob/main/LICENSE
ShortDescription: Save disk space with the compression built into Windows, from Explorer's right-click menu.
Description: |-
  Scrimp saves disk space using the compression already built into NTFS. Right-click a file, a folder or a whole drive in Explorer and choose Scrimp Compress: your files stay exactly as they are, just smaller on disk, and apps open them as normal. Scrimp Decompress undoes it at any time.
  It also adds the scrimp command, for the same from a terminal or a script.
Moniker: scrimp
Tags:
  - compression
  - disk-space
  - explorer
  - ntfs
  - wof
ReleaseNotesUrl: {REPO}/releases/tag/v{version}
ManifestType: defaultLocale
ManifestVersion: {MANIFEST_VERSION}
",
        header("defaultLocale")
    );
    vec![
        (format!("{IDENTIFIER}.yaml"), version_file),
        (format!("{IDENTIFIER}.installer.yaml"), installer_file),
        (format!("{IDENTIFIER}.locale.en-US.yaml"), locale_file),
    ]
}

/// The installer's checksum, from a `SHA256SUMS` file.
pub fn installer_sum(sums: &str) -> Result<String> {
    let sum = sums
        .lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == INSTALLER)
        .map(|(sum, _)| sum.to_string())
        .with_context(|| format!("SHA256SUMS has no line for {INSTALLER}"))?;
    if sum.len() != 64 || !sum.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("SHA256SUMS's line for {INSTALLER} isn't a SHA-256 checksum: {sum}");
    }
    Ok(sum)
}

/// Writes the manifests into `out`, in the folders winget-pkgs keeps them
/// in (`manifests/m/MarkCastle/Scrimp/<version>`), and returns that folder.
pub fn write(version: &str, sums: &str, release_date: &str, out: &Path) -> Result<PathBuf> {
    let folder = out
        .join("manifests")
        .join("m")
        .join("MarkCastle")
        .join("Scrimp")
        .join(version);
    std::fs::create_dir_all(&folder).with_context(|| format!("Couldn't make {}", folder.display()))?;
    for (name, text) in manifests(version, &installer_sum(sums)?, release_date) {
        std::fs::write(folder.join(&name), text).with_context(|| format!("Couldn't write {name}"))?;
    }
    Ok(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUM: &str = "64b0aabc0031a2a7376c3cfd0b7eaeb8cdbac5d9cbc96391ff899abb96ed0858";

    fn sums() -> String {
        format!(
            "16dbf2bccaed181f27d1b51d9675de29d91b794505522861a781a389b8132b60  scrimp-windows-x64.exe\n{SUM}  Scrimp-windows-x64-setup.exe\n"
        )
    }

    #[test]
    fn the_installer_checksum_is_read_from_the_release() {
        assert_eq!(installer_sum(&sums()).unwrap(), SUM);
        assert_eq!(
            installer_sum(&format!("{SUM} *Scrimp-windows-x64-setup.exe")).unwrap(),
            SUM
        );
        assert!(installer_sum("abc  scrimp-windows-x64.exe\n").is_err());
        assert!(installer_sum("not-hex  Scrimp-windows-x64-setup.exe\n").is_err());
    }

    #[test]
    fn the_manifests_describe_this_release_and_how_to_install_it_quietly() {
        let files = manifests("0.3.0", SUM, "2026-11-27");
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "MarkCastle.Scrimp.yaml",
                "MarkCastle.Scrimp.installer.yaml",
                "MarkCastle.Scrimp.locale.en-US.yaml"
            ]
        );
        for (name, text) in &files {
            assert!(text.contains("PackageIdentifier: MarkCastle.Scrimp\n"), "{name}");
            assert!(text.contains("PackageVersion: 0.3.0\n"), "{name}");
            assert!(text.ends_with("ManifestVersion: 1.6.0\n"), "{name}");
            assert!(!text.contains('\t'), "YAML takes no tabs: {name}");
        }
        let installer = &files[1].1;
        assert!(installer.contains(
            "InstallerUrl: https://github.com/markcastle/scrimp/releases/download/v0.3.0/Scrimp-windows-x64-setup.exe\n"
        ));
        assert!(installer.contains(&format!("InstallerSha256: {}\n", SUM.to_ascii_uppercase())));
        assert!(installer.contains("InstallerType: nullsoft\nScope: user\n"));
        assert!(installer.contains("ProductCode: Scrimp\n"));
        assert!(files[2].1.contains("License: MIT\n"));
    }

    #[test]
    fn they_land_where_winget_pkgs_keeps_them() {
        let dir = tempfile::tempdir().unwrap();
        let folder = write("0.3.0", &sums(), "2026-11-27", dir.path()).unwrap();
        assert!(folder.ends_with("manifests/m/MarkCastle/Scrimp/0.3.0"));
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 3);
    }
}
