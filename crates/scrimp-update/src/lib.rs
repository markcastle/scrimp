//! Updating Scrimp to the newest release. Adapted from Keevoro's
//! `keevoro-update`, so both projects update the same way.
//!
//! `scrimp update` asks GitHub for the latest release, and if it's newer
//! than the running copy, fetches what this computer needs, checks it, and
//! puts it in place:
//!
//! - **Installed with the installer** (an `uninstall.exe` sits beside
//!   `scrimp.exe`): it downloads the new installer and starts it quietly,
//!   telling it to wait for this program to exit. The installer then
//!   replaces everything, the Explorer menu included.
//! - **Installed with `install.ps1`** (the command line on its own): it
//!   downloads the new `scrimp.exe` and swaps it in. Windows lets a running
//!   program be renamed, so the old one moves aside and the new one takes
//!   its name.
//!
//! Three rules, kept from Keevoro:
//!
//! - **Only ever move up.** A release that isn't strictly higher than the
//!   running version is ignored, which is why a rollback is released under
//!   a higher number.
//! - **Check before using.** Every release carries a `SHA256SUMS` file, and
//!   a download that doesn't match it is thrown away.
//! - **Never phone home unasked.** GitHub is only contacted when the person
//!   runs `scrimp update`.
//!
//! The network sits behind the small [`Web`] trait so the whole flow is
//! tested without one.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

pub const REPO: &str = "markcastle/scrimp";
pub const SUMS: &str = "SHA256SUMS";
/// The command line on its own.
pub const COMMAND_LINE: &str = "scrimp-windows-x64.exe";
/// The installer.
pub const INSTALLER: &str = "Scrimp-windows-x64-setup.exe";
/// No Scrimp file is anywhere near this; a larger download is refused.
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(
        "Couldn't find a Scrimp release. If the repository has been made private, set GITHUB_TOKEN to a token that can read it"
    )]
    NotFound,
    #[error("GitHub refused the request. Check that GITHUB_TOKEN is still valid and can read the repository")]
    Refused,
    #[error("Couldn't reach GitHub ({0}). Check your internet connection and try again")]
    Network(String),
    #[error("GitHub sent something unexpected ({0})")]
    Unexpected(String),
    #[error("Release {version} doesn't include {file}")]
    Missing { version: String, file: String },
    #[error("The download didn't match the release's checksum, so it was thrown away. Nothing was changed. Try again")]
    BadChecksum,
    #[error(
        "Couldn't put the new version in place ({0}). Nothing was changed. Running the installer from the release page will update Scrimp"
    )]
    Install(String),
}

/// The two things the updater needs from the internet.
pub trait Web {
    /// A small JSON or text document.
    fn text(&self, url: &str) -> Result<String, UpdateError>;
    /// A release file. `url` is the asset's API address.
    fn bytes(&self, url: &str) -> Result<Vec<u8>, UpdateError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Without the leading `v`.
    pub version: String,
    /// The release's page, where its notes are.
    pub page: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    UpToDate,
    Available(Release),
}

/// How the running copy was installed, which decides how it's updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// With the installer, into this folder.
    ByInstaller(PathBuf),
    /// With `install.ps1`: just this `scrimp.exe`.
    CommandLineOnly(PathBuf),
}

impl Installed {
    /// Works out how `exe` (the running `scrimp.exe`) was installed.
    pub fn of(exe: &Path) -> Installed {
        match exe.parent() {
            Some(folder) if folder.join("uninstall.exe").is_file() => Installed::ByInstaller(folder.to_path_buf()),
            _ => Installed::CommandLineOnly(exe.to_path_buf()),
        }
    }

    /// The release file this kind of install needs.
    pub fn file(&self) -> &'static str {
        match self {
            Installed::ByInstaller(_) => INSTALLER,
            Installed::CommandLineOnly(_) => COMMAND_LINE,
        }
    }
}

/// Reads the reply from GitHub's "latest release" address.
pub fn parse_release(json: &str) -> Result<Release, UpdateError> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| UpdateError::Unexpected(e.to_string()))?;
    let text = |value: &serde_json::Value, key: &str| value.get(key).and_then(|x| x.as_str()).map(str::to_string);
    let tag = text(&v, "tag_name").ok_or_else(|| UpdateError::Unexpected("no tag_name".into()))?;
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some(Asset {
                        name: text(a, "name")?,
                        url: text(a, "url")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        version: tag.trim_start_matches('v').to_string(),
        page: text(&v, "html_url").unwrap_or_else(|| format!("https://github.com/{REPO}/releases")),
        assets,
    })
}

/// Is `candidate` a strictly higher version than `current`? Compared as
/// numbers, so 0.10.0 is after 0.9.0. Anything unreadable is not newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn numbers(v: &str) -> Option<[u64; 3]> {
        let mut parts = v
            .trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| p.parse::<u64>().ok());
        let out = [parts.next()??, parts.next()??, parts.next()??];
        parts.next().is_none().then_some(out)
    }
    matches!((numbers(candidate), numbers(current)), (Some(a), Some(b)) if a > b)
}

/// Asks whether a newer release than `current` exists.
pub fn check(web: &dyn Web, current: &str) -> Result<Check, UpdateError> {
    let release = parse_release(&web.text(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?)?;
    if is_newer(&release.version, current) {
        Ok(Check::Available(release))
    } else {
        Ok(Check::UpToDate)
    }
}

/// Downloads one file from `release` and proves it matches the release's
/// checksum file.
pub fn download(web: &dyn Web, release: &Release, name: &str) -> Result<Vec<u8>, UpdateError> {
    let find = |wanted: &str| release.assets.iter().find(|a| a.name == wanted);
    let asset = find(name).ok_or_else(|| UpdateError::Missing {
        version: release.version.clone(),
        file: name.to_string(),
    })?;
    // A release without checksums is not one this updater will trust.
    let sums = find(SUMS).ok_or(UpdateError::BadChecksum)?;
    let sums = String::from_utf8(web.bytes(&sums.url)?).map_err(|_| UpdateError::BadChecksum)?;
    let expected = expected_sum(&sums, name).ok_or(UpdateError::BadChecksum)?;
    let bytes = web.bytes(&asset.url)?;
    if sha256_hex(&bytes) != expected {
        return Err(UpdateError::BadChecksum);
    }
    Ok(bytes)
}

/// Finds a file's checksum in `sha256sum` output: `<hex>  <name>` per line.
pub fn expected_sum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| hash.to_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Puts a new `scrimp.exe` in place of `exe`, which may be running. The
/// old copy is renamed to `scrimp.old` (Windows allows renaming a running
/// program) and the new one takes its name. If any step fails, the old copy
/// is put back.
pub fn replace_program(exe: &Path, program: &[u8]) -> Result<(), UpdateError> {
    let fail = |e: std::io::Error| UpdateError::Install(e.to_string());
    let new = exe.with_extension("new");
    let old = exe.with_extension("old");
    std::fs::write(&new, program).map_err(fail)?;
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(fail(e));
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(fail(e));
    }
    Ok(())
}

/// Removes the copy a previous update moved aside, once it's no longer running.
pub fn clean_up(exe: &Path) {
    let _ = std::fs::remove_file(exe.with_extension("old"));
}

/// Saves the installer and starts it quietly into `folder`, telling it to
/// wait for this process to exit first. Doesn't wait for it.
pub fn start_installer(installer: &[u8], folder: &Path) -> Result<(), UpdateError> {
    let fail = |e: std::io::Error| UpdateError::Install(e.to_string());
    let path = std::env::temp_dir().join(format!("Scrimp-update-{}-setup.exe", std::process::id()));
    std::fs::write(&path, installer).map_err(fail)?;
    std::process::Command::new(&path)
        .arg("/S")
        .arg(format!("/WAITPID={}", std::process::id()))
        // NSIS reads /D= to the end of the line, unquoted, so it goes last.
        .arg(format!("/D={}", folder.display()))
        .spawn()
        .map_err(fail)?;
    Ok(())
}

/// GitHub over HTTPS. A token is only needed while the repository is
/// private; it is sent to GitHub's API and nowhere else.
pub struct GitHub {
    agent: ureq::Agent,
    token: Option<String>,
}

impl GitHub {
    pub fn new(token: Option<String>) -> Self {
        // Trust what Windows trusts, not a list baked into the program.
        // Offices and schools often route traffic through a proxy with its
        // own certificate; a built-in list would reject it.
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15 * 60)))
            .tls_config(tls)
            .build()
            .into();
        Self {
            agent,
            token: token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        }
    }

    /// Reads the token from `SCRIMP_GITHUB_TOKEN` or `GITHUB_TOKEN`.
    pub fn from_env() -> Self {
        Self::new(
            std::env::var("SCRIMP_GITHUB_TOKEN")
                .or_else(|_| std::env::var("GITHUB_TOKEN"))
                .ok(),
        )
    }

    fn get(&self, url: &str, accept: &str) -> Result<ureq::http::Response<ureq::Body>, UpdateError> {
        let mut request = self
            .agent
            .get(url)
            .header("User-Agent", "scrimp-update")
            .header("Accept", accept);
        if let Some(token) = &self.token {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        request.call().map_err(|e| match e {
            ureq::Error::StatusCode(404) => UpdateError::NotFound,
            ureq::Error::StatusCode(401 | 403) => UpdateError::Refused,
            ureq::Error::StatusCode(code) => UpdateError::Unexpected(format!("status {code}")),
            other => UpdateError::Network(other.to_string()),
        })
    }
}

impl Web for GitHub {
    fn text(&self, url: &str) -> Result<String, UpdateError> {
        self.get(url, "application/vnd.github+json")?
            .body_mut()
            .read_to_string()
            .map_err(|e| UpdateError::Network(e.to_string()))
    }

    fn bytes(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        self.get(url, "application/octet-stream")?
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD)
            .read_to_vec()
            .map_err(|e| UpdateError::Network(e.to_string()))
    }
}

#[cfg(test)]
mod tests;
