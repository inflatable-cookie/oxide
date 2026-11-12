//! Derives `b_mac::Who` with the full path, exactly as Relax KNOWN_GAPS #1's
//! repro does (oxide#307).

#[derive(mac_b::Who)]
pub struct InB;

/// The recorded verdict of this crate's derive.
pub fn in_b_who() -> &'static str {
    InB::who()
}
