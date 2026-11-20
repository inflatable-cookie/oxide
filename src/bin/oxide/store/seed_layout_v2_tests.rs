//! Layout v2 cold-slot seeding against real Cargo 1.100.0 output. A donor
//! slot is built by Cargo, published as a catalog and released idle. A cold
//! recipient slot then seeds the registry library unit, and Cargo must report
//! it fresh instead of compiling it again. Cargo still judges every copy: an
//! aged dep-info or a changed profile makes it rebuild.
//!
//! The Cargo home is a fresh fixture under the test root. The invoking home
//! holds the one archive this test locks in two index directories, and seeding
//! refuses an ambiguous registry index, so the fixture copies that archive and
//! its index entry (read only) into a home that has exactly one. The test
//! skips with a message when Cargo 1.100.0 or those cache entries are absent.

use super::seeding::SeedRun;
use super::tests::{lease, seed_fixture};
use super::*;

use crate::cargo_layout::support::{cargo_1_100, toolchain_installed, TOOLCHAIN};
use crate::freshness::Built;
use crate::identity::SlotKey;
use crate::process;
use crate::seed_context::{self, ProducerFacts};
use crate::seed_plan::VerifiedSource;
use oxide_storage::lease::managed::{SeedCatalog, SeedProducer};
use oxide_storage::lease::seed::ADAPTER_V2;
use oxide_storage::seed_source::SourceCache;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

const MANIFEST: &str = "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nitoa = \"=1.0.18\"\n";
const LIB: &str =
    "pub fn f() -> String { let mut b = itoa::Buffer::new(); b.format(7u32).to_string() }\n";
const MAIN: &str = "fn main() { println!(\"{}\", probe::f()); }\n";
/// The one registry package this project locks, in the index directory Cargo
/// uses for crates.io.
const INDEX: &str = "index.crates.io-1949cf8c6b5b557f";
const ARCHIVE: &str = "itoa-1.0.18.crate";
const INDEX_ENTRY: &str = ".cache/it/oa/itoa";

fn invoking_cargo_home() -> PathBuf {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".cargo"))
}

/// A fresh Cargo home under `root` holding exactly one registry archive and its
/// index entry, copied from `invoking`. Nothing under `invoking` is written.
fn fixture_cargo_home(root: &Path, invoking: &Path) -> PathBuf {
    let home = root.join("cargo-home");
    let cache = home.join("registry/cache").join(INDEX);
    let index = home.join("registry/index").join(INDEX);
    fs::create_dir_all(&cache).unwrap();
    fs::create_dir_all(index.join(".cache/it/oa")).unwrap();
    fs::copy(
        invoking.join("registry/cache").join(INDEX).join(ARCHIVE),
        cache.join(ARCHIVE),
    )
    .unwrap();
    for name in ["config.json", ".last-updated"] {
        let from = invoking.join("registry/index").join(INDEX).join(name);
        if from.is_file() {
            fs::copy(from, index.join(name)).unwrap();
        }
    }
    fs::copy(
        invoking
            .join("registry/index")
            .join(INDEX)
            .join(INDEX_ENTRY),
        index.join(INDEX_ENTRY),
    )
    .unwrap();
    home
}

/// A change to a seeded copy that Cargo must notice.
enum Damage {
    MissingArtifact,
    AlteredFingerprint,
}

fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), MANIFEST).unwrap();
    fs::write(dir.join("src/lib.rs"), LIB).unwrap();
    fs::write(dir.join("src/main.rs"), MAIN).unwrap();
}

/// One offline `cargo build` in `checkout`, with the slot's build directory as
/// the front door sets it, the target directory in the checkout and the test
/// Cargo home.
fn cargo_build(checkout: &Path, build: &Path, home: &Path) -> Output {
    cargo_1_100(checkout)
        .args(["build", "-v", "--offline"])
        .env("CARGO_HOME", home)
        .env("CARGO_BUILD_BUILD_DIR", build)
        .env("CARGO_TARGET_DIR", checkout.join("target"))
        .output()
        .expect("cargo 1.100.0")
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
fn recipient(root: &Path, name: &str, store: &Store, lock_bytes: &[u8]) -> (SlotKey, Lease) {
    let checkout = root.join(name);
    write_project(&checkout);
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
    let invoking = invoking_cargo_home();
    let index = invoking.join("registry/index").join(INDEX);
    if !index.join(INDEX_ENTRY).is_file()
        || !invoking
            .join("registry/cache")
            .join(INDEX)
            .join(ARCHIVE)
            .is_file()
    {
        eprintln!("skipped: {ARCHIVE} is not in the invoking Cargo registry cache");
        return;
    }
    let (root, store, donor_key) = seed_fixture();
    let home = fixture_cargo_home(&root, &invoking);
    let started = SystemTime::now();
    let started_ms = process::now_ms();

    // The donor: one real Cargo 1.100.0 build, catalogued and released idle.
    let donor = lease(&store, &donor_key, None).expect("donor lease");
    write_project(&donor_key.checkout);
    let built = cargo_build(&donor_key.checkout, &donor.build_dir, &home);
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
    assert!(unit_dir.starts_with("debug/build/itoa/"), "{unit_dir}");
    store.release(donor, Some(origin)).expect("donor release");

    // A cold recipient seeds the unit, and Cargo reuses it.
    let (fresh_key, fresh) = recipient(&root, "fresh", &store, &lock_bytes);
    let mut fresh_cache = SourceCache::new(&fresh.store_id).unwrap();
    let seeded = store
        .seed(&fresh, &producer, &lock, |_| {
            Ok(verified(&home, &lock, &mut fresh_cache))
        })
        .expect("the recipient seed");
    match seeded {
        SeedRun::Seeded(..) => {}
        SeedRun::Skipped(reason) => panic!("seeding skipped: {reason}"),
        SeedRun::Abandoned => panic!("seeding abandoned"),
    }
    assert!(fresh
        .build_dir
        .join(&unit_dir)
        .join("fingerprint/dep-lib-itoa")
        .is_file());
    let output = cargo_build(&fresh_key.checkout, &fresh.build_dir, &home);
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Fresh itoa v1.0.18"), "{stderr}");
    assert!(!stderr.contains("Compiling itoa"), "{stderr}");
    assert!(stderr.contains("Compiling probe"), "{stderr}");
    // The copied unit links: the binary runs with the seeded library.
    let run = Command::new(fresh_key.checkout.join("target/debug/probe"))
        .output()
        .expect("probe binary");
    assert_eq!(text(&run.stdout).trim(), "7");
    store.release(fresh, None).expect("fresh release");

    // Cargo judges the copy: a changed profile is a new unit, so it compiles.
    let (changed_key, changed) = recipient(&root, "changed", &store, &lock_bytes);
    let mut manifest = MANIFEST.to_owned();
    manifest.push_str("\n[profile.dev]\nopt-level = 1\n");
    fs::write(changed_key.checkout.join("Cargo.toml"), manifest).unwrap();
    let mut changed_cache = SourceCache::new(&changed.store_id).unwrap();
    let seeded = store
        .seed(&changed, &producer, &lock, |_| {
            Ok(verified(&home, &lock, &mut changed_cache))
        })
        .expect("the changed seed");
    assert!(matches!(seeded, SeedRun::Seeded(..)));
    let output = cargo_build(&changed_key.checkout, &changed.build_dir, &home);
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Compiling itoa"), "{stderr}");
    store.release(changed, None).expect("changed release");

    // Cargo judges the copy by its own state. An incomplete copy, a missing
    // artifact, and an altered fingerprint hash are each rebuilt, never reused.
    for (name, damage) in [
        ("missing", Damage::MissingArtifact),
        ("altered", Damage::AlteredFingerprint),
    ] {
        let (key, lease) = recipient(&root, name, &store, &lock_bytes);
        let mut cache = SourceCache::new(&lease.store_id).unwrap();
        let seeded = store
            .seed(&lease, &producer, &lock, |_| {
                Ok(verified(&home, &lock, &mut cache))
            })
            .expect("the damaged seed");
        assert!(matches!(seeded, SeedRun::Seeded(..)));
        let unit = lease.build_dir.join(&unit_dir);
        match damage {
            Damage::MissingArtifact => {
                fs::remove_file(unit.join("out/libitoa-497cf91033786c66.rmeta")).unwrap()
            }
            Damage::AlteredFingerprint => {
                fs::write(unit.join("fingerprint/lib-itoa"), b"0").unwrap()
            }
        }
        let output = cargo_build(&key.checkout, &lease.build_dir, &home);
        let stderr = text(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(stderr.contains("Compiling itoa"), "{name}: {stderr}");
        store.release(lease, None).expect("damaged release");
    }

    fs::remove_dir_all(root).unwrap();
}
