//! The KNOWN_GAPS #1 collision probe (oxide#307): `app` imports `mac_a::Who`
//! by bare name while `lib_b` derives the same macro name through its own
//! full path. rustc must print `app=a b=b`; a bare-name registry keyed by the
//! macro's name prints `app=a b=a`, which the driver records as the known
//! upstream gap, never as a passed case.

use mac_a::Who;

#[derive(Who)]
struct InApp;

fn main() {
    println!("app={} b={}", InApp::who(), lib_b::in_b_who());
}
