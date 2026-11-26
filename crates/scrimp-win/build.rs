//! Embeds tests.manifest into scrimp-win's test program, so the task dialog
//! tests in src/ui.rs can run: the dialogs need Common Controls v6, which a
//! program only gets from its manifest. A library is never linked on its
//! own, so these arguments reach only its own tests, never scrimp.exe or
//! scrimpw.exe. Only for the MSVC toolchain on Windows.

fn main() {
    println!("cargo:rerun-if-changed=tests.manifest");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        let manifest =
            std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join("tests.manifest");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
}
