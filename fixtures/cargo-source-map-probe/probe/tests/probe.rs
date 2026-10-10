use oxide_source_probe::generated;
use oxide_source_probe::{INCLUDED_VALUE, INCLUDE_INPUT_PATH, MANIFEST_DIRECTORY, OUT_DIRECTORY};
use std::fs;
use std::panic;

const PROC_MACRO: (&str, &str) = oxide_source_probe_macro::observe_marker!();

#[test]
fn emits_observed_paths_and_values() -> Result<(), Box<dyn std::error::Error>> {
    let runtime_path = std::path::Path::new(MANIFEST_DIRECTORY).join("marker.txt");
    let runtime_value = fs::read_to_string(&runtime_path)?;
    let generated_path = std::path::Path::new(OUT_DIRECTORY).join("generated.rs");
    let generated_source = fs::read_to_string(&generated_path)?;
    let marker = INCLUDED_VALUE.trim();

    assert_eq!(generated::BUILD_VALUE, marker);
    assert_eq!(PROC_MACRO.1, marker);
    assert_eq!(runtime_value.trim(), marker);
    assert!(generated_source.contains(marker));

    println!("OXIDE_PROBE\tmanifest-directory\t{MANIFEST_DIRECTORY}");
    println!("OXIDE_PROBE\tinclude-input-path\t{INCLUDE_INPUT_PATH}");
    println!("OXIDE_PROBE\tinclude-value\t{marker}");
    println!(
        "OXIDE_PROBE\tbuild-input-path\t{}",
        generated::BUILD_INPUT_PATH
    );
    println!("OXIDE_PROBE\tbuild-value\t{}", generated::BUILD_VALUE);
    println!("OXIDE_PROBE\tout-directory\t{OUT_DIRECTORY}");
    println!(
        "OXIDE_PROBE\tgenerated-output-path\t{}",
        generated::GENERATED_OUTPUT_PATH
    );
    println!("OXIDE_PROBE\tgenerated-value\t{}", generated::BUILD_VALUE);
    println!("OXIDE_PROBE\tproc-macro-input-path\t{}", PROC_MACRO.0);
    println!("OXIDE_PROBE\tproc-macro-value\t{}", PROC_MACRO.1);
    println!(
        "OXIDE_PROBE\truntime-resource-path\t{}",
        runtime_path.display()
    );
    println!("OXIDE_PROBE\truntime-value\t{}", runtime_value.trim());
    println!("OXIDE_PROBE\tfile-macro\t{}", file!());

    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(|info| {
        if let Some(location) = info.location() {
            println!(
                "OXIDE_PROBE\tpanic-location\t{}:{}",
                location.file(),
                location.line()
            );
        }
    }));
    let panic_result = panic::catch_unwind(|| panic!("observed source-map location"));
    panic::set_hook(previous_hook);
    assert!(panic_result.is_err());
    Ok(())
}
