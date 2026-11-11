//! Prints the build script's generated constant. Expected observable line:
//! `generated=gen-constant-v1`.

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

fn main() {
    println!("generated={GENERATED}");
}
