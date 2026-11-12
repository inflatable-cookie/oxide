//! Float-formatting correspondence probe for the Relax compatibility recheck
//! (oxide#308).
//!
//! Every input is a declared bit pattern. For each value the program prints
//! the input bits, `serde_json`'s serialized text, and the bits decoded back
//! from that text; the program exits nonzero if any decoded value differs from
//! its input, so both arms self-validate without any external expectation.
//! `serde_json` on aarch64 formats through `zmij`, the crate the old Relax pin
//! could not compile: this is the actual digit path, not an "imports exist"
//! check. The declared values cover signs, negative zero, fractions, the
//! scientific-notation thresholds, both finite extremes, normal/subnormal
//! boundaries and values whose shortest representation rounds.
//!
//! Nothing here reads the clock, the environment, the filesystem or random
//! state, so the whole stdout is a deterministic function of the source.

use serde_json::{from_str, to_string};

/// f64 inputs, by bit pattern (sign, subnormal, finite extremes, rounding).
const F64_BITS: [u64; 30] = [
    0x0000_0000_0000_0000, // 0.0
    0x8000_0000_0000_0000, // -0.0
    0x3ff0_0000_0000_0000, // 1.0
    0xbff0_0000_0000_0000, // -1.0
    0x3fe0_0000_0000_0000, // 0.5
    0x3fb9_9999_9999_999a, // 0.1
    0x3fd3_3333_3333_3333, // 0.3
    0x3fd5_5555_5555_5555, // 1/3
    0x4009_21fb_5444_2d18, // pi
    0x3ff0_0000_0000_0001, // next after 1.0
    0x3fef_ffff_ffff_ffff, // next below 1.0
    0x4008_0000_0000_0001, // next after 3.0
    0x4340_0000_0000_0000, // 2^53
    0x43e0_0000_0000_0000, // 2^63
    0x7fef_ffff_ffff_ffff, // f64::MAX
    0xffef_ffff_ffff_ffff, // f64::MIN
    0x0010_0000_0000_0000, // MIN_POSITIVE (smallest normal)
    0x000f_ffff_ffff_ffff, // largest subnormal
    0x000f_ffff_ffff_fffe, // second largest subnormal
    0x0008_0000_0000_0000, // mid subnormal
    0x0000_0000_0000_0001, // smallest subnormal
    0x0010_0000_0000_0002, // next after MIN_POSITIVE
    0x7fe0_0000_0000_0000, // ~8.99e307
    0x44b5_2d02_c7e1_4af6, // 1e23
    0x3e7a_d7f2_9abc_af48, // 1e-7
    0xbe7a_d7f2_9abc_af48, // -1e-7
    0x437b_69b4_ba63_0f35, // 1.2345678901234568e17
    0x4341_c379_37e0_8000, // 1e16
    0x3ee4_f8b5_88e3_68f1, // 1e-5
    0x4024_0000_0000_0000, // 10.0
];

/// f32 inputs, by bit pattern.
const F32_BITS: [u32; 18] = [
    0x0000_0000, // 0.0
    0x8000_0000, // -0.0
    0x3f80_0000, // 1.0
    0xbf80_0000, // -1.0
    0x3f00_0000, // 0.5
    0x3e99_999a, // 0.3
    0x3dcc_cccd, // 0.1
    0x3eaa_aaab, // 1/3
    0x4049_0fdb, // pi
    0x3f80_0001, // next after 1.0
    0x4b80_0000, // 2^24
    0x7f7f_ffff, // f32::MAX
    0xff7f_ffff, // f32::MIN
    0x0080_0000, // MIN_POSITIVE
    0x007f_ffff, // largest subnormal
    0x0000_0001, // smallest subnormal
    0x006c_e3ee, // 1e-38
    0x7eff_c99e, // 1.7e38
];

/// Decimal inputs for the decode path, including the classic hard roundings.
const PARSE_F64: [&str; 8] = [
    "2.2250738585072011e-308",
    "5e-324",
    "1.7976931348623157e308",
    "-0.0",
    "9007199254740993",
    "0.1",
    "1e-7",
    "2.225073858507201e-308",
];

const PARSE_F32: [&str; 4] = ["1e-45", "3.4028235e38", "-0.0", "0.3"];

fn main() {
    let mut failures = 0u32;
    println!(
        "declared f64={} f32={} parse_f64={} parse_f32={}",
        F64_BITS.len(),
        F32_BITS.len(),
        PARSE_F64.len(),
        PARSE_F32.len()
    );

    for (index, bits) in F64_BITS.iter().enumerate() {
        let value = f64::from_bits(*bits);
        let text = match to_string(&value) {
            Ok(text) => text,
            Err(error) => {
                failures += 1;
                println!("f64 {index} {bits:016x} error {error}");
                continue;
            }
        };
        let decoded = match from_str::<f64>(&text) {
            Ok(value) => value.to_bits(),
            Err(error) => {
                failures += 1;
                println!("f64 {index} {bits:016x} {text} error {error}");
                continue;
            }
        };
        if decoded != *bits {
            failures += 1;
        }
        println!("f64 {index} {bits:016x} {text} {decoded:016x}");
    }

    for (index, bits) in F32_BITS.iter().enumerate() {
        let value = f32::from_bits(*bits);
        let text = match to_string(&value) {
            Ok(text) => text,
            Err(error) => {
                failures += 1;
                println!("f32 {index} {bits:08x} error {error}");
                continue;
            }
        };
        let decoded = match from_str::<f32>(&text) {
            Ok(value) => value.to_bits(),
            Err(error) => {
                failures += 1;
                println!("f32 {index} {bits:08x} {text} error {error}");
                continue;
            }
        };
        if decoded != *bits {
            failures += 1;
        }
        println!("f32 {index} {bits:08x} {text} {decoded:08x}");
    }

    for (index, text) in PARSE_F64.iter().enumerate() {
        let value = match from_str::<f64>(text) {
            Ok(value) => value,
            Err(error) => {
                failures += 1;
                println!("parse-f64 {index} {text} error {error}");
                continue;
            }
        };
        let bits = value.to_bits();
        let again = match to_string(&value) {
            Ok(text) => text,
            Err(error) => {
                failures += 1;
                println!("parse-f64 {index} {text} {bits:016x} error {error}");
                continue;
            }
        };
        let round = match from_str::<f64>(&again) {
            Ok(value) => value.to_bits(),
            Err(error) => {
                failures += 1;
                println!("parse-f64 {index} {text} {bits:016x} error {error}");
                continue;
            }
        };
        if round != bits {
            failures += 1;
        }
        println!("parse-f64 {index} {text} {bits:016x} {again} {round:016x}");
    }

    for (index, text) in PARSE_F32.iter().enumerate() {
        let value = match from_str::<f32>(text) {
            Ok(value) => value,
            Err(error) => {
                failures += 1;
                println!("parse-f32 {index} {text} error {error}");
                continue;
            }
        };
        let bits = value.to_bits();
        let again = match to_string(&value) {
            Ok(text) => text,
            Err(error) => {
                failures += 1;
                println!("parse-f32 {index} {text} {bits:08x} error {error}");
                continue;
            }
        };
        let round = match from_str::<f32>(&again) {
            Ok(value) => value.to_bits(),
            Err(error) => {
                failures += 1;
                println!("parse-f32 {index} {text} {bits:08x} error {error}");
                continue;
            }
        };
        if round != bits {
            failures += 1;
        }
        println!("parse-f32 {index} {text} {bits:08x} {again} {round:08x}");
    }

    let checked = F64_BITS.len() + F32_BITS.len() + PARSE_F64.len() + PARSE_F32.len();
    println!("checked={checked} failures={failures}");
    if failures != 0 {
        std::process::exit(1);
    }
}
