//! Generated-output correspondence: a tiny build script writes code the
//! binary includes. The expected observable line is fixed in the driver.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let dest = Path::new(&out_dir).join("generated.rs");
    fs::write(
        &dest,
        "pub const GENERATED: &str = \"gen-constant-v1\";\n",
    )
    .expect("write generated.rs");
    println!("cargo:rerun-if-changed=build.rs");
}
