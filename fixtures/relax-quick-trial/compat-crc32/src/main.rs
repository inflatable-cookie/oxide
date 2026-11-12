//! CRC32 correspondence probe for the Relax compatibility recheck (oxide#308).
//!
//! Every input is a fixed byte sequence built in-process by a deterministic
//! xorshift64 generator (no clock, no randomness, no filesystem). The program
//! prints one hex digest per named pattern, the digest of a 7-byte incremental
//! fold over the long buffer (exercising the streaming `Hasher` API), and the
//! standard check value for `123456789` so the probe has one independent
//! expected constant instead of only a cross-arm comparison.
//!
//! The 8200-byte buffer crosses crc32fast's 192-quad three-way interleaved
//! path, so the exercised shape is the multi-stream one, not only the short
//! input path.

use crc32fast::Hasher;

fn xorshift_bytes(length: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut out = Vec::with_capacity(length);
    for _ in 0..length {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state >> 24) as u8);
    }
    out
}

fn main() {
    let long = xorshift_bytes(8200);
    let zeros = vec![0u8; 4096];
    let fox = b"the quick brown fox jumps over the lazy dog";
    let patterns: [(&str, &[u8]); 5] = [
        ("empty", b""),
        ("check", b"123456789"),
        ("fox", fox),
        ("long", &long),
        ("zeros", &zeros),
    ];

    for (index, (name, bytes)) in patterns.iter().enumerate() {
        println!("crc32 {index} {name} {:08x}", crc32fast::hash(bytes));
    }

    let mut hasher = Hasher::new();
    for chunk in long.chunks(7) {
        hasher.update(chunk);
    }
    println!("crc32-fold long {:08x}", hasher.finalize());

    let mut split = Hasher::new();
    split.update(b"123456789");
    println!("crc32-stream check {:08x}", split.finalize());
    println!("crc32-check 123456789 cbf43926");
}
