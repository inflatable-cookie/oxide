//! Debug-assertion correspondence: in the quick (dev) profile `u8 + 10` on
//! 250 must panic with the overflow message and exit 101. The trial never
//! disables overflow checks to make an arm pass.

fn main() {
    let base: u8 = 250;
    let overflowed = base + 10;
    println!("no overflow: {overflowed}");
}
