//! Bounded human-readable descriptions of recorded seed facts.

use oxide_storage::lease::managed::{SeedLast, SeedRejectReason};
use std::path::Path;

/// Read a recorded seed only after the front door has enabled qualified
/// seeding. The disabled branch performs no sidecar read.
pub(crate) fn recorded_if_enabled(pool: &Path, enabled: bool) -> Option<String> {
    recorded_with(enabled, || {
        oxide_storage::lease::managed::read_seed_last(pool)
    })
}

fn recorded_with(enabled: bool, read: impl FnOnce() -> Option<SeedLast>) -> Option<String> {
    if !enabled {
        return None;
    }
    read().map(|last| recorded(&last))
}

fn reason(reason: SeedRejectReason) -> &'static str {
    match reason {
        SeedRejectReason::AmbiguousName => "ambiguous name",
        SeedRejectReason::NonLeaf => "non-leaf",
        SeedRejectReason::JoinUnprovable => "join unprovable",
        SeedRejectReason::UnverifiedSource => "unverified source",
        SeedRejectReason::UnsupportedSource => "unsupported source",
        SeedRejectReason::RunOnly => "run-only",
        SeedRejectReason::Cap => "cap",
    }
}

fn suffix(last: &SeedLast) -> String {
    let total: u64 = last
        .buckets
        .iter()
        .map(|bucket| u64::from(bucket.units))
        .sum();
    let mut parts: Vec<String> = last
        .buckets
        .iter()
        .map(|bucket| format!("{} ({})", reason(bucket.reason), bucket.units))
        .collect();
    if last.unknown > 0 {
        parts.push(format!("unknown ({})", last.unknown));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(
            "; skipped {total} donor candidate units: {}",
            parts.join(", ")
        )
    }
}

fn bounded(mut line: String) -> String {
    if line.len() > 240 {
        let mut end = 237;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push_str("...");
    }
    line
}

pub(crate) fn summary(last: &SeedLast) -> String {
    bounded(format!(
        "oxide: seeded {} units ({:.1} MiB) from slot {}{}",
        last.units,
        last.logical_bytes as f64 / (1024.0 * 1024.0),
        last.donor_slot,
        suffix(last)
    ))
}

pub(crate) fn recorded(last: &SeedLast) -> String {
    bounded(format!(
        "  recorded seed: slot {}, from slot {}, {} units ({:.1} MiB), {} files, {} ms, at {}{}",
        last.slot_id,
        last.donor_slot,
        last.units,
        last.logical_bytes as f64 / (1024.0 * 1024.0),
        last.files,
        last.elapsed_ms,
        last.when_ms,
        suffix(last)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_storage::lease::managed::SeedBucket;

    #[test]
    fn disabled_status_never_reads_the_sidecar() {
        assert!(recorded_with(false, || panic!("unexpected sidecar read")).is_none());
        assert!(recorded_with(true, || None).is_none());
    }

    #[test]
    fn summary_distinguishes_unknown_from_counted_units() {
        let mut last = SeedLast {
            schema: "oxide.cold-seed/1".into(),
            store_id: "s".into(),
            slot_id: "1".into(),
            incarnation: "i".into(),
            lease_id: "l".into(),
            donor_slot: "2".into(),
            units: 1,
            files: 3,
            logical_bytes: 0,
            elapsed_ms: 1,
            when_ms: 2,
            buckets: vec![SeedBucket {
                reason: SeedRejectReason::AmbiguousName,
                units: 2,
            }],
            unknown: 1,
        };
        assert!(summary(&last)
            .contains("skipped 2 donor candidate units: ambiguous name (2), unknown (1)"));
        assert!(recorded(&last).contains("ambiguous name (2), unknown (1)"));
        last.buckets.clear();
        last.unknown = 0;
        assert!(!summary(&last).contains("skipped"));
        assert!(recorded(&last).len() <= 240);
    }
}
