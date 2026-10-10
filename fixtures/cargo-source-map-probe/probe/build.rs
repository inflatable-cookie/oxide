use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let input = manifest.join("marker.txt");
    let value = fs::read_to_string(&input)?;
    let value = value.trim();
    let generated = out_dir.join("generated.rs");
    let macro_input = out_dir.join("proc-macro-input.txt");
    let source = format!(
        "pub const BUILD_INPUT_PATH: &str = {input:?};\npub const BUILD_VALUE: &str = {value:?};\npub const GENERATED_OUTPUT_PATH: &str = {generated:?};\npub const OUT_DIR_VALUE: &str = {out_dir:?};\n"
    );
    fs::write(&generated, source)?;
    fs::write(&macro_input, value)?;
    fs::write(
        out_dir.join("producer-observation.txt"),
        format!(
            "{}\n{}\n{}\n{}\n",
            input.display(),
            generated.display(),
            macro_input.display(),
            out_dir.display(),
        ),
    )?;
    println!("cargo:rerun-if-changed=marker.txt");
    Ok(())
}
