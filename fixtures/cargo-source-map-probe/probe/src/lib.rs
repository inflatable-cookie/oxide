pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

pub const MANIFEST_DIRECTORY: &str = env!("CARGO_MANIFEST_DIR");
pub const OUT_DIRECTORY: &str = env!("OUT_DIR");
pub const INCLUDE_INPUT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/marker.txt");
pub const INCLUDED_VALUE: &str = include_str!("../marker.txt");
