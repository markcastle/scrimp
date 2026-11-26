//! Ported from keevoro-update's tests, plus Scrimp's own: how it was
//! installed, and swapping the program.

use super::*;
use std::collections::BTreeMap;

/// A pretend GitHub holding one latest release.
struct FakeWeb {
    latest: Result<String, ()>,
    files: BTreeMap<String, Vec<u8>>,
}

impl Web for FakeWeb {
    fn text(&self, url: &str) -> Result<String, UpdateError> {
        assert_eq!(url, "https://api.github.com/repos/markcastle/scrimp/releases/latest");
        self.latest.clone().map_err(|()| UpdateError::NotFound)
    }
    fn bytes(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        self.files
            .get(url)
            .cloned()
            .ok_or_else(|| UpdateError::Network("gone".into()))
    }
}

const PROGRAM: &[u8] = b"pretend this is a scrimp file";

/// A release as GitHub describes it, with both of Scrimp's files.
fn github(tag: &str) -> FakeWeb {
    let mut files = BTreeMap::new();
    let mut assets = Vec::new();
    let mut sums = String::new();
    for (i, name) in [COMMAND_LINE, INSTALLER].iter().enumerate() {
        let url = format!("https://api.github.com/repos/markcastle/scrimp/releases/assets/{i}");
        files.insert(url.clone(), PROGRAM.to_vec());
        assets.push(serde_json::json!({ "name": name, "url": url }));
        sums.push_str(&format!("{}  {name}\n", sha256_hex(PROGRAM)));
    }
    let sums_url = "https://api.github.com/repos/markcastle/scrimp/releases/assets/99".to_string();
    files.insert(sums_url.clone(), sums.into_bytes());
    assets.push(serde_json::json!({ "name": SUMS, "url": sums_url }));
    let latest = serde_json::json!({
        "tag_name": tag,
        "html_url": format!("https://github.com/markcastle/scrimp/releases/tag/{tag}"),
        "assets": assets,
    });
    FakeWeb {
        latest: Ok(latest.to_string()),
        files,
    }
}

fn available(web: &FakeWeb, current: &str) -> Release {
    match check(web, current).unwrap() {
        Check::Available(r) => r,
        Check::UpToDate => panic!("expected an update from {current}"),
    }
}

fn url_of(release: &Release, name: &str) -> String {
    release.assets.iter().find(|a| a.name == name).unwrap().url.clone()
}

#[test]
fn a_higher_release_is_offered_and_nothing_else_is() {
    let web = github("v0.3.0");
    let release = available(&web, "0.2.5");
    assert_eq!(release.version, "0.3.0");
    assert_eq!(release.page, "https://github.com/markcastle/scrimp/releases/tag/v0.3.0");
    assert_eq!(check(&web, "0.3.0").unwrap(), Check::UpToDate);
    assert_eq!(check(&web, "0.4.0").unwrap(), Check::UpToDate, "never move down");
    assert_eq!(
        check(&github("nightly"), "0.1.0").unwrap(),
        Check::UpToDate,
        "an odd tag is not an update"
    );
}

#[test]
fn versions_compare_as_numbers() {
    assert!(is_newer("0.10.0", "0.9.9"));
    assert!(is_newer("v1.0.0", "0.99.0"));
    assert!(is_newer("0.2.1", "0.2.0"));
    assert!(!is_newer("0.2.0", "0.2.0"));
    assert!(!is_newer("0.9.0", "0.10.0"));
    for odd in ["", "1.2", "1.2.3.4", "one.two.three", "1.2.3-beta"] {
        assert!(!is_newer(odd, "0.0.1"), "{odd}");
        assert!(!is_newer("9.9.9", odd), "{odd}");
    }
}

#[test]
fn good_downloads_are_handed_over_and_damaged_ones_thrown_away() {
    let mut web = github("v0.3.0");
    let release = available(&web, "0.2.0");
    assert_eq!(download(&web, &release, COMMAND_LINE).unwrap(), PROGRAM);
    assert_eq!(download(&web, &release, INSTALLER).unwrap(), PROGRAM);
    // Cut short on the way down.
    web.files.insert(url_of(&release, INSTALLER), PROGRAM[..10].to_vec());
    let err = download(&web, &release, INSTALLER).unwrap_err();
    assert!(matches!(err, UpdateError::BadChecksum));
    assert!(err.to_string().contains("Nothing was changed"));
    assert!(
        download(&web, &release, COMMAND_LINE).is_ok(),
        "the other file is unaffected"
    );
}

#[test]
fn a_release_without_usable_checksums_is_not_trusted() {
    let web = github("v0.3.0");
    let mut release = available(&web, "0.2.0");
    let sums_url = url_of(&release, SUMS);

    let mut partial = github("v0.3.0");
    partial
        .files
        .insert(sums_url.clone(), b"abc  something-else\n".to_vec());
    assert!(matches!(
        download(&partial, &release, COMMAND_LINE),
        Err(UpdateError::BadChecksum)
    ));

    let mut garbage = github("v0.3.0");
    garbage.files.insert(sums_url, vec![0xFF, 0xFE, 0x00]);
    assert!(matches!(
        download(&garbage, &release, COMMAND_LINE),
        Err(UpdateError::BadChecksum)
    ));

    release.assets.retain(|a| a.name != SUMS);
    assert!(matches!(
        download(&web, &release, COMMAND_LINE),
        Err(UpdateError::BadChecksum)
    ));
}

#[test]
fn a_missing_file_says_which() {
    let web = github("v0.3.0");
    let mut release = available(&web, "0.2.0");
    release.assets.retain(|a| a.name != INSTALLER);
    let err = download(&web, &release, INSTALLER).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Release 0.3.0 doesn't include Scrimp-windows-x64-setup.exe"
    );
}

#[test]
fn network_trouble_is_passed_on_plainly() {
    let mut web = github("v0.3.0");
    let release = available(&web, "0.2.0");
    web.files.clear();
    assert!(matches!(
        download(&web, &release, COMMAND_LINE),
        Err(UpdateError::Network(_))
    ));
    web.latest = Err(());
    let err = check(&web, "0.2.0").unwrap_err();
    assert!(err.to_string().contains("set GITHUB_TOKEN"), "{err}");
}

#[test]
fn checksum_files_are_read_the_way_sha256sum_writes_them() {
    let hash = sha256_hex(b"abc");
    assert_eq!(hash, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    let sums = format!(
        "{}  other-file\n{}  {COMMAND_LINE}\n{} *binary-mode\n",
        "0".repeat(64),
        hash.to_uppercase(),
        hash
    );
    assert_eq!(expected_sum(&sums, COMMAND_LINE).unwrap(), hash, "case doesn't matter");
    assert_eq!(expected_sum(&sums, "binary-mode").unwrap(), hash);
    assert_eq!(expected_sum(&sums, "scrimp"), None, "a name must match whole");
    assert_eq!(expected_sum(&format!("tooshort  {COMMAND_LINE}\n"), COMMAND_LINE), None);
}

#[test]
fn odd_replies_from_github_dont_crash() {
    assert!(matches!(
        parse_release("<html>rate limited</html>"),
        Err(UpdateError::Unexpected(_))
    ));
    assert!(matches!(parse_release("{}"), Err(UpdateError::Unexpected(_))));
    let bare =
        parse_release(r#"{"tag_name":"v1.2.3","assets":[{"name":"x"},{"url":"y"},{"name":"ok","url":"u"}]}"#).unwrap();
    assert_eq!(bare.version, "1.2.3");
    assert_eq!(bare.page, "https://github.com/markcastle/scrimp/releases");
    assert_eq!(
        bare.assets,
        [Asset {
            name: "ok".into(),
            url: "u".into()
        }]
    );
}

#[test]
fn how_scrimp_was_installed_decides_what_to_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("scrimp.exe");
    std::fs::write(&exe, "").unwrap();
    assert_eq!(Installed::of(&exe), Installed::CommandLineOnly(exe.clone()));
    assert_eq!(Installed::of(&exe).file(), COMMAND_LINE);
    std::fs::write(dir.path().join("uninstall.exe"), "").unwrap();
    assert_eq!(Installed::of(&exe), Installed::ByInstaller(dir.path().to_path_buf()));
    assert_eq!(Installed::of(&exe).file(), INSTALLER);
}

#[test]
fn the_program_is_swapped_and_the_old_copy_tidied_later() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("scrimp.exe");
    std::fs::write(&exe, "version one").unwrap();
    replace_program(&exe, b"version two").unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"version two");
    assert_eq!(std::fs::read(exe.with_extension("old")).unwrap(), b"version one");
    assert!(!exe.with_extension("new").exists());
    // A second update replaces the leftover from the first.
    replace_program(&exe, b"version three").unwrap();
    assert_eq!(std::fs::read(exe.with_extension("old")).unwrap(), b"version two");
    clean_up(&exe);
    assert!(!exe.with_extension("old").exists());
    clean_up(&exe);
}

#[test]
fn a_failed_swap_leaves_the_old_program() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("scrimp.exe");
    // There is no program to move aside, so the swap can't happen.
    let err = replace_program(&exe, b"version two").unwrap_err();
    assert!(matches!(err, UpdateError::Install(_)));
    assert!(!exe.exists() && !exe.with_extension("new").exists());
}

#[test]
fn a_token_is_optional_and_tidied() {
    assert_eq!(
        GitHub::new(Some("  ghp_abc \n".into())).token.as_deref(),
        Some("ghp_abc")
    );
    assert_eq!(GitHub::new(Some("   ".into())).token, None);
    assert_eq!(GitHub::new(None).token, None);
}

#[test]
fn an_address_that_isnt_listening_is_a_network_problem() {
    let web = GitHub::new(Some("token".into()));
    assert!(matches!(
        web.text("http://127.0.0.1:9/nothing"),
        Err(UpdateError::Network(_))
    ));
    assert!(matches!(
        web.bytes("http://127.0.0.1:9/nothing"),
        Err(UpdateError::Network(_))
    ));
}

/// A one-request web server on this machine, standing in for GitHub so the
/// real HTTP code is exercised. Returns its address and what it was sent.
fn serve_once(status: &str, body: &'static [u8]) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let status = status.to_string();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
        }
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        String::from_utf8_lossy(&request).to_lowercase()
    });
    (address, handle)
}

#[test]
fn requests_carry_the_token_and_ask_for_the_right_thing() {
    let (address, sent) = serve_once("200 OK", b"{\"tag_name\":\"v0.3.0\"}");
    let web = GitHub::new(Some("ghp_secret".into()));
    assert_eq!(
        web.text(&format!("{address}/latest")).unwrap(),
        "{\"tag_name\":\"v0.3.0\"}"
    );
    let request = sent.join().unwrap();
    assert!(request.contains("authorization: bearer ghp_secret"), "{request}");
    assert!(request.contains("accept: application/vnd.github+json"), "{request}");
    assert!(request.contains("user-agent: scrimp-update"), "{request}");

    let (address, sent) = serve_once("200 OK", PROGRAM);
    let anonymous = GitHub::new(None);
    assert_eq!(anonymous.bytes(&format!("{address}/asset")).unwrap(), PROGRAM);
    let request = sent.join().unwrap();
    assert!(request.contains("accept: application/octet-stream"), "{request}");
    assert!(!request.contains("authorization"), "no token, no header: {request}");
}

#[test]
fn githubs_refusals_are_explained() {
    let web = GitHub::new(None);
    for (status, expected) in [
        ("404 Not Found", "set GITHUB_TOKEN"),
        ("401 Unauthorized", "GITHUB_TOKEN is still valid"),
        ("403 Forbidden", "GITHUB_TOKEN is still valid"),
        ("500 Internal Server Error", "something unexpected (status 500)"),
    ] {
        let (address, sent) = serve_once(status, b"{}");
        let err = web.text(&format!("{address}/latest")).unwrap_err().to_string();
        assert!(err.contains(expected), "{status}: {err}");
        sent.join().unwrap();
    }
}
