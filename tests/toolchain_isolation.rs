//! A private test HOME must not cause rustup to install another toolchain.

use std::ffi::{CStr, CString, OsStr};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let template = std::env::temp_dir().join("oxide-rustup-isolation.XXXXXX");
        let mut path = CString::new(template.as_os_str().as_bytes())
            .unwrap()
            .into_bytes_with_nul();
        // SAFETY: the owned buffer is writable, NUL-terminated and ends in XXXXXX.
        assert!(!unsafe { libc::mkdtemp(path.as_mut_ptr().cast()) }.is_null());
        let name = CStr::from_bytes_until_nul(&path).unwrap();
        Self(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove owned test scratch");
    }
}

#[test]
fn private_home_uses_the_invoking_rustup_home() {
    // Capture before replacing HOME, including the explicit invocation value.
    let invoking_home = std::env::var_os("HOME").expect("invoking HOME");
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(invoking_home).join(".rustup"));
    let rustup_home = fs::canonicalize(rustup_home).expect("installed RUSTUP_HOME");
    let scratch = Scratch::new();
    let home = scratch.0.join("home");
    let cargo_home = scratch.0.join("cargo-home");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&cargo_home).unwrap();
    let status = Command::new("rustc")
        .arg("--version")
        .env("HOME", &home)
        .env("CARGO_HOME", &cargo_home)
        .env("RUSTUP_HOME", &rustup_home)
        .env("RUSTUP_TOOLCHAIN", "1.97.1")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .status()
        .expect("run installed rustc");
    assert!(status.success());
    assert!(!home.join(".rustup").exists());
}
