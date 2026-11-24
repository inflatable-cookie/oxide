//! Layout v2 cold-slot seeding against real Cargo 1.100.0 output. A donor
//! slot is built by Cargo, published as a catalog and released idle. A cold
//! recipient slot then seeds the registry library unit, and Cargo must report
//! it fresh instead of compiling it again. A changed profile, a missing
//! artifact or an altered fingerprint makes Cargo rebuild.
//!
//! The fixture uses an unambiguous registry package in the invoking Cargo home.

use super::seeding::SeedRun;
use super::tests::{lease, seed_fixture};
use super::*;

use crate::cargo_layout::support::{cargo_1_100, toolchain_installed, TOOLCHAIN};
use crate::freshness::Built;
use crate::identity::SlotKey;
use crate::process;
use crate::seed_context::{self, ProducerFacts};
use crate::seed_plan::VerifiedSource;
use oxide_storage::lease::managed::{SeedCatalog, SeedFileKind, SeedProducer};
use oxide_storage::lease::seed::ADAPTER_V2;
use oxide_storage::seed_source::{RegistrySource, SourceCache};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

const LIB: &str =
    "pub fn f() -> String { let mut b = itoa::Buffer::new(); b.format(7u32).to_string() }\n";
const MAIN: &str = "fn main() { println!(\"{}\", probe::f()); }\n";

/// A change to a seeded copy that Cargo must notice.
enum Damage {
    MissingArtifact,
    AlteredFingerprint,
}

fn write_project(dir: &Path, manifest: &str) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    fs::write(dir.join("src/lib.rs"), LIB).unwrap();
    fs::write(dir.join("src/main.rs"), MAIN).unwrap();
}

/// One offline `cargo build` in `checkout`, with the slot's build directory as
/// the front door sets it and the target directory in the checkout.
fn cargo_build(checkout: &Path, build: &Path) -> Output {
    cargo_1_100(checkout)
        .args(["build", "-v", "--offline"])
        .env("CARGO_BUILD_BUILD_DIR", build)
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TARGET_DIR", checkout.join("target"))
        .output()
        .expect("cargo 1.100.0")
}

fn unit_files(unit: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    ["fingerprint", "out"]
        .into_iter()
        .flat_map(|subdir| fs::read_dir(unit.join(subdir)).unwrap())
        .map(|entry| {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(unit).unwrap().to_owned();
            (relative, fs::read(path).unwrap())
        })
        .collect()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The layout v2 producer identity that the donor and the recipients record.
fn producer(checkout: &Path, home: &Path) -> SeedProducer {
    let cargo = text(
        &cargo_1_100(checkout)
            .arg("-V")
            .output()
            .expect("cargo -V")
            .stdout,
    );
    let rustc = text(
        &Command::new("rustup")
            .args(["run", TOOLCHAIN, "rustc", "-vV"])
            .output()
            .expect("rustc -vV")
            .stdout,
    );
    let target = rustc
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("the host triple")
        .to_owned();
    let context = seed_context::collect(checkout, home, &[], &[]).expect("producer context");
    seed_context::producer(
        ProducerFacts {
            cargo_version: cargo.trim(),
            rustc_version: &rustc,
            rustc: None,
            target: &target,
            recipe: "quick",
            backend: "directory-budget-v1",
            cargo_home: home,
            command: "build",
        },
        context,
    )
    .expect("layout v2 producer")
}

fn verified(
    home: &Path,
    lock: &crate::seed_lock::Analysis,
    cache: &mut SourceCache,
) -> Vec<VerifiedSource> {
    crate::seed_sources::verify_pass(home, &lock.eligible, cache).expect("registry sources verify")
}

/// A cold recipient in its own checkout, with the same lock as the donor.
fn recipient(
    root: &Path,
    name: &str,
    store: &Store,
    manifest: &str,
    lock_bytes: &[u8],
) -> (SlotKey, Lease) {
    let checkout = root.join(name);
    write_project(&checkout, manifest);
    fs::write(checkout.join("Cargo.lock"), lock_bytes).unwrap();
    let key = SlotKey {
        checkout,
        identity: name.into(),
        compatibility: "quick-key".into(),
    };
    let lease = lease(store, &key, None).expect("recipient lease");
    (key, lease)
}

#[test]
fn a_cold_1_100_slot_seeds_from_an_idle_donor_and_cargo_reuses_the_unit() {
    if !toolchain_installed() {
        eprintln!("skipped: Cargo {TOOLCHAIN} is not installed");
        return;
    }
    let home = crate::cargo_home::resolve()
        .unwrap()
        .canonicalize()
        .unwrap();
    // Prefer the workspace's locked version. The alternate is available on
    // hosts whose legacy index cache makes that version ambiguous.
    let Some(version) = ["1.0.18", "1.0.17"]
        .into_iter()
        .find(|version| RegistrySource::locate(&home, &format!("itoa-{version}")).is_ok())
    else {
        eprintln!("skipped: neither pinned itoa version has an unambiguous cached registry source");
        return;
    };
    let manifest = format!(
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nitoa = \"={version}\"\n\n\
         [profile.dev]\ndebug = 2\nsplit-debuginfo = \"unpacked\"\n"
    );
    let (root, store, donor_key) = seed_fixture();
    let started = SystemTime::now();
    let started_ms = process::now_ms();

    // The donor: one real Cargo 1.100.0 build, catalogued and released idle.
    let donor = lease(&store, &donor_key, None).expect("donor lease");
    write_project(&donor_key.checkout, &manifest);
    let built = cargo_build(&donor_key.checkout, &donor.build_dir);
    assert!(built.status.success(), "{}", text(&built.stderr));
    let lock_bytes = fs::read(donor_key.checkout.join("Cargo.lock")).unwrap();
    let lock = crate::seed_lock::analyze(&lock_bytes).unwrap();
    let producer = producer(&donor_key.checkout, &home);
    assert_eq!(producer.adapter, ADAPTER_V2);
    let mut donor_cache = SourceCache::new(&donor.store_id).unwrap();
    let sources = verified(&home, &lock, &mut donor_cache);
    assert_eq!(sources.len(), 1, "the one registry source verifies");
    let origin = Built::Affine(donor_key.checkout.clone());
    store
        .publish_seed_catalog(
            &donor,
            producer.clone(),
            &lock,
            &sources,
            &sources,
            started,
            started_ms,
            true,
            Some(&origin),
            "debug",
            &donor_cache,
        )
        .expect("the donor publishes its catalog");
    let catalog_path = store
        .control
        .join("seeds")
        .join(format!("{}.catalog.json", donor.slot_id));
    let catalog = SeedCatalog::parse(&fs::read(&catalog_path).expect("donor catalog")).unwrap();
    let units: Vec<_> = catalog
        .packages
        .iter()
        .flat_map(|package| &package.units)
        .collect();
    assert_eq!(units.len(), 1, "one library unit for itoa: {catalog:?}");
    let unit_dir = units[0].fingerprint_dir.clone();
    // The rmeta path carries the unit hash, which the test does not predict.
    let artifact = units[0]
        .files
        .iter()
        .find(|file| file.kind == SeedFileKind::Artifact && file.path.ends_with(".rmeta"))
        .expect("the rmeta artifact")
        .path
        .clone();
    assert!(unit_dir.starts_with("debug/build/itoa/"), "{unit_dir}");
    let expected = unit_files(&donor.build_dir.join(&unit_dir));
    assert!(expected
        .keys()
        .any(|path| path.extension().is_some_and(|ext| ext == "o")));
    store.release(donor, Some(origin)).expect("donor release");

    // A cold recipient seeds the unit, and Cargo reuses it.
    let (fresh_key, fresh) = recipient(&root, "fresh", &store, &manifest, &lock_bytes);
    let mut fresh_cache = SourceCache::new(&fresh.store_id).unwrap();
    let seeded = store
        .seed(&fresh, &producer, &lock, |_| {
            Ok(verified(&home, &lock, &mut fresh_cache))
        })
        .expect("the recipient seed");
    match seeded {
        SeedRun::Seeded(..) => {}
        SeedRun::Skipped(reason) => panic!("seeding skipped: {reason}"),
        SeedRun::Abandoned(reason) => panic!("seeding abandoned: {}", reason.as_str()),
    }
    assert!(
        expected == unit_files(&fresh.build_dir.join(&unit_dir)),
        "the seeded unit matches every donor file"
    );
    let output = cargo_build(&fresh_key.checkout, &fresh.build_dir);
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!("Fresh itoa v{version}")),
        "{stderr}"
    );
    assert!(!stderr.contains("Compiling itoa"), "{stderr}");
    assert!(stderr.contains("Compiling probe"), "{stderr}");
    // The copied unit links: the binary runs with the seeded library.
    let run = Command::new(fresh_key.checkout.join("target/debug/probe"))
        .output()
        .expect("probe binary");
    assert_eq!(text(&run.stdout).trim(), "7");
    store.release(fresh, None).expect("fresh release");

    // Cargo judges the copy: a changed profile is a new unit, so it compiles.
    let changed_manifest = format!("{manifest}opt-level = 1\n");
    let (changed_key, changed) =
        recipient(&root, "changed", &store, &changed_manifest, &lock_bytes);
    let mut changed_cache = SourceCache::new(&changed.store_id).unwrap();
    let seeded = store
        .seed(&changed, &producer, &lock, |_| {
            Ok(verified(&home, &lock, &mut changed_cache))
        })
        .expect("the changed seed");
    assert!(matches!(seeded, SeedRun::Seeded(..)));
    let output = cargo_build(&changed_key.checkout, &changed.build_dir);
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Compiling itoa"), "{stderr}");
    store.release(changed, None).expect("changed release");

    // Cargo also rebuilds a unit with a missing artifact or altered fingerprint.
    for (name, damage) in [
        ("missing", Damage::MissingArtifact),
        ("altered", Damage::AlteredFingerprint),
    ] {
        let (key, lease) = recipient(&root, name, &store, &manifest, &lock_bytes);
        let mut cache = SourceCache::new(&lease.store_id).unwrap();
        let seeded = store
            .seed(&lease, &producer, &lock, |_| {
                Ok(verified(&home, &lock, &mut cache))
            })
            .expect("the damaged seed");
        assert!(matches!(seeded, SeedRun::Seeded(..)));
        let unit = lease.build_dir.join(&unit_dir);
        match damage {
            Damage::MissingArtifact => fs::remove_file(lease.build_dir.join(&artifact)).unwrap(),
            Damage::AlteredFingerprint => {
                fs::write(unit.join("fingerprint/lib-itoa"), b"0").unwrap()
            }
        }
        let output = cargo_build(&key.checkout, &lease.build_dir);
        let stderr = text(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(stderr.contains("Compiling itoa"), "{name}: {stderr}");
        store.release(lease, None).expect("damaged release");
    }

    fs::remove_dir_all(root).unwrap();
}
