//! Exercise the quick dependency debug override through real Cargo and rustc.

use super::{fake_cargo, private_env, Scratch};
use std::fs;
use std::path::PathBuf;
use std::process::Output;

#[test]
fn incompatible_profiles_and_exec_stop_before_cargo_or_pool_setup() {
    let scratch = Scratch::new();
    let (bin, log) = fake_cargo(scratch.path(), &scratch.path().join("Cargo.toml"));
    let pool = scratch.path().join("pool");
    for args in [
        &["check", "--profile", "debug", "--no-dep-debug"][..],
        &["check", "--profile", "release", "--no-dep-debug"],
        &["check", "--release", "--no-dep-debug"],
        &["exec", "--no-dep-debug", "--", "cargo", "check"],
    ] {
        let output = private_env(scratch.path(), &bin, &pool)
            .current_dir(scratch.path())
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?} succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("applies only to quick Cargo commands"),
            "{args:?}: {stderr}"
        );
        assert!(!log.exists(), "{args:?} invoked Cargo");
        assert!(!pool.exists(), "{args:?} created a pool");
    }
}

#[test]
fn quick_without_dependency_debug_preserves_workspace_feedback() {
    let scratch = Scratch::new();
    let root = scratch.path();
    let app = root.join("app");
    let pool = root.join("pool");
    for dir in ["src", "tests", "dependency/src"] {
        fs::create_dir_all(app.join(dir)).unwrap();
    }
    fs::write(
        app.join("Cargo.toml"),
        "[package]\nname = 'profile-app'\nversion = '0.0.0'\nedition = '2024'\n\
         [dependencies]\nprofile-dep = { path = 'dependency' }\n\
         [workspace]\nexclude = ['dependency']\n",
    )
    .unwrap();
    fs::write(
        app.join("dependency/Cargo.toml"),
        "[package]\nname = 'profile-dep'\nversion = '0.0.0'\nedition = '2024'\n",
    )
    .unwrap();
    fs::write(
        app.join("dependency/src/lib.rs"),
        "pub fn value() -> u8 { 42 }\n",
    )
    .unwrap();
    fs::write(
        app.join("src/main.rs"),
        "fn main() {\n    assert_eq!(std::env::args().nth(1).as_deref(), Some(\"--no-dep-debug\"));\n    println!(\"{}\", profile_dep::value());\n}\n",
    )
    .unwrap();
    fs::write(
        app.join("tests/profile_backtrace.rs"),
        include_str!("../profile_backtrace.rs"),
    )
    .unwrap();

    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".rustup"));
    let invoke = |args: &[&str]| -> Output {
        let output = private_env(root, &root.join("bin"), &pool)
            .current_dir(&app)
            .env("RUSTUP_HOME", &rustup_home)
            .env("RUSTUP_TOOLCHAIN", "1.100.0")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("OXIDE_POOL_CREATE", "1")
            .env("OXIDE_POOL_CAP", "512MiB")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "oxide {args:?} failed ({}):\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output
    };

    invoke(&["check", "--no-dep-debug", "--offline"]);
    let baseline = invoke(&["build", "--offline", "-vv"]);
    assert!(rustc_line(&baseline, "profile_dep").contains("debuginfo=line-tables-only"));
    let opted_in = invoke(&["build", "--no-dep-debug", "--offline", "-vv"]);
    let dependency = rustc_line(&opted_in, "profile_dep");
    assert!(
        !dependency.contains("-C debuginfo="),
        "dependency compiler arguments: {dependency}"
    );
    assert!(rustc_line(&opted_in, "profile_app").contains("debuginfo=line-tables-only"));

    let run = invoke(&["run", "--no-dep-debug", "--offline", "--", "--no-dep-debug"]);
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
    let test = invoke(&[
        "test",
        "--no-dep-debug",
        "--offline",
        "--test",
        "profile_backtrace",
        "--",
        "--exact",
        "profile_probe_keeps_workspace_frames_and_unwind",
    ]);
    assert!(String::from_utf8_lossy(&test.stdout).contains("1 passed"));
}

fn rustc_line(output: &Output, crate_name: &str) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let name = format!("--crate-name {crate_name} ");
    stderr
        .lines()
        .find(|line| line.contains(&name))
        .unwrap_or_else(|| panic!("no rustc invocation for {crate_name}:\n{stderr}"))
        .to_owned()
}
