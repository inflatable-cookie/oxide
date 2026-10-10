use super::source_map::MAX_COMPILE_ORIGINS;
use crate::storage::SlotState;
use std::collections::VecDeque;

pub const MAX_EVENTS_PER_RUN: usize = 12;
pub const MAX_EVENT_BYTES: usize = 1024;
pub const OBSERVATION_RING_BYTES: usize = 16 * 1024 * 1024;

/// Write attribution is unavailable unless a supported counter is supplied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteAttribution {
    Unavailable,
}

/// Correlated, non-authoritative metadata for one managed fixture invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunObservation {
    pub project_component: String,
    pub consumer: String,
    pub task_id: String,
    pub workspace_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub host_run_id: Option<String>,
    pub workflow: String,
    pub selector: String,
    pub commit: String,
    pub source_tree_digest: String,
    pub partition_digest: String,
    pub config_digest: String,
    pub source_map_policy_digest: Option<String>,
    pub source_map_digest: Option<String>,
    pub compile_origin_tree_digests: Vec<String>,
    pub backend: &'static str,
    pub events: Vec<ObservationEvent>,
    pub(super) settled: bool,
}

/// One fixed-schema observation. Values contain no command lines or raw environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationEvent {
    pub sequence: u8,
    pub retry_count: u16,
    pub kind: ObservationKind,
}

/// Bounded lifecycle data emitted by the project adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservationKind {
    Requested {
        operation_id: String,
    },
    Refused {
        reason: &'static str,
    },
    Acquired {
        operation_id: String,
        reused: bool,
    },
    Storage {
        accounted_bytes: Option<u64>,
        free_bytes: Option<u64>,
        reserved_bytes: u64,
        write_attribution: WriteAttribution,
    },
    HostRunSubmitted {
        host_run_id: String,
    },
    WorkCompleted {
        elapsed_ms: u64,
        compile_elapsed_ms: Option<u64>,
        test_elapsed_ms: Option<u64>,
        reused_units: Option<u32>,
        rebuilt_units: Option<u32>,
        exit_code: Option<i32>,
        cancelled: bool,
    },
    CaptureCompleted {
        reference: String,
        digest: String,
        elapsed_ms: u64,
    },
    SourceMapQualified {
        receipt_digest: String,
        binary_set_digest: String,
    },
    CaptureUnavailable,
    Finalized {
        operation_id: String,
        state: SlotState,
    },
    Uncertain {
        reason: &'static str,
    },
}

impl ObservationKind {
    fn operation(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::Requested { operation_id } => Some(("request", operation_id)),
            Self::Acquired { operation_id, .. } => Some(("acquire", operation_id)),
            Self::Finalized { operation_id, .. } => Some(("finalize", operation_id)),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(super) struct ObservationRing {
    runs: VecDeque<RunObservation>,
    used_bytes: usize,
    dropped: u64,
}

impl ObservationRing {
    pub fn snapshot(&self) -> Vec<RunObservation> {
        self.runs.iter().cloned().collect()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn begin(&mut self, run: RunObservation) -> bool {
        if self
            .runs
            .iter()
            .any(|existing| observation_key(existing) == observation_key(&run))
        {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        let bytes = encoded_len(&run);
        if bytes > OBSERVATION_RING_BYTES || !self.make_room(bytes, None) {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.used_bytes = self.used_bytes.saturating_add(bytes);
        self.runs.push_back(run);
        true
    }

    pub fn event(&mut self, consumer: &str, run_id: &str, attempt_id: &str, kind: ObservationKind) {
        let key = observation_key_parts(consumer, run_id, attempt_id);
        let operation = kind.operation();
        let Some(run_index) = self.runs.iter().position(|run| observation_key(run) == key) else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        if let Some((tag, operation_id)) = operation {
            let event_index = self.runs[run_index]
                .events
                .iter()
                .position(|event| event.kind.operation() == Some((tag, operation_id)));
            if let Some(event_index) = event_index {
                let mut candidate = self.runs[run_index].clone();
                candidate.events[event_index].retry_count =
                    candidate.events[event_index].retry_count.saturating_add(1);
                let delta =
                    encoded_len(&candidate).saturating_sub(encoded_len(&self.runs[run_index]));
                if self.make_room(delta, Some(&key)) {
                    if let Some(run) = self.runs.iter_mut().find(|run| observation_key(run) == key)
                    {
                        run.events[event_index].retry_count =
                            run.events[event_index].retry_count.saturating_add(1);
                    }
                    self.used_bytes = self.used_bytes.saturating_add(delta);
                } else {
                    self.dropped = self.dropped.saturating_add(1);
                }
                return;
            }
        }
        let run = &self.runs[run_index];
        if run.events.len() >= MAX_EVENTS_PER_RUN {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let event = ObservationEvent {
            sequence: run.events.len() as u8 + 1,
            retry_count: 0,
            kind,
        };
        if encoded_len(&event) > MAX_EVENT_BYTES {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let mut candidate = run.clone();
        candidate.events.push(event.clone());
        let delta = encoded_len(&candidate).saturating_sub(encoded_len(run));
        if self.make_room(delta, Some(&key)) {
            if let Some(run) = self.runs.iter_mut().find(|run| observation_key(run) == key) {
                run.events.push(event);
            }
            self.used_bytes = self.used_bytes.saturating_add(delta);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub fn set_host_run(
        &mut self,
        consumer: &str,
        run_id: &str,
        attempt_id: &str,
        host_run_id: String,
    ) {
        let key = observation_key_parts(consumer, run_id, attempt_id);
        let Some(index) = self.runs.iter().position(|run| observation_key(run) == key) else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        let mut candidate = self.runs[index].clone();
        candidate.host_run_id = Some(host_run_id.clone());
        let delta = encoded_len(&candidate).saturating_sub(encoded_len(&self.runs[index]));
        if self.make_room(delta, Some(&key)) {
            if let Some(run) = self.runs.iter_mut().find(|run| observation_key(run) == key) {
                run.host_run_id = Some(host_run_id);
            }
            self.used_bytes = self.used_bytes.saturating_add(delta);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub fn set_compile_origins(
        &mut self,
        consumer: &str,
        run_id: &str,
        attempt_id: &str,
        origins: Vec<String>,
    ) {
        if origins.len() > MAX_COMPILE_ORIGINS {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let key = observation_key_parts(consumer, run_id, attempt_id);
        let Some(index) = self.runs.iter().position(|run| observation_key(run) == key) else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        let mut candidate = self.runs[index].clone();
        candidate.compile_origin_tree_digests = origins;
        let delta = encoded_len(&candidate).saturating_sub(encoded_len(&self.runs[index]));
        if self.make_room(delta, Some(&key)) {
            if let Some(run) = self.runs.iter_mut().find(|run| observation_key(run) == key) {
                run.compile_origin_tree_digests = candidate.compile_origin_tree_digests;
            }
            self.used_bytes = self.used_bytes.saturating_add(delta);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub fn settle(&mut self, consumer: &str, run_id: &str, attempt_id: &str) {
        let key = observation_key_parts(consumer, run_id, attempt_id);
        let settled =
            if let Some(run) = self.runs.iter_mut().find(|run| observation_key(run) == key) {
                run.settled = true;
                true
            } else {
                false
            };
        if settled {
            self.recount();
        }
    }

    fn make_room(&mut self, added: usize, protected_run: Option<&str>) -> bool {
        while self.used_bytes.saturating_add(added) > OBSERVATION_RING_BYTES {
            let oldest = self.runs.iter().position(|run| {
                run.settled && Some(observation_key(run).as_str()) != protected_run
            });
            let Some(index) = oldest else {
                return false;
            };
            if let Some(removed) = self.runs.remove(index) {
                self.used_bytes = self.used_bytes.saturating_sub(encoded_len(&removed));
            }
        }
        true
    }

    fn recount(&mut self) {
        self.used_bytes = self.runs.iter().map(encoded_len).sum();
    }
}

fn observation_key(run: &RunObservation) -> String {
    observation_key_parts(&run.consumer, &run.run_id, &run.attempt_id)
}

fn observation_key_parts(consumer: &str, run_id: &str, attempt_id: &str) -> String {
    format!(
        "{}:{}{}:{}{}",
        consumer.len(),
        consumer,
        run_id.len(),
        run_id,
        attempt_id
    )
}

fn encoded_len<T: std::fmt::Debug>(value: &T) -> usize {
    format!("{:?}", value).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(run_id: &str) -> RunObservation {
        RunObservation {
            project_component: "project:component".to_owned(),
            consumer: "fixture".to_owned(),
            task_id: "task".to_owned(),
            workspace_id: "workspace".to_owned(),
            run_id: run_id.to_owned(),
            attempt_id: format!("attempt-{}", run_id),
            host_run_id: None,
            workflow: "rust:test".to_owned(),
            selector: "test:fixture".to_owned(),
            commit: "d".repeat(40),
            source_tree_digest: format!("sha256:{}", "a".repeat(64)),
            partition_digest: format!("sha256:{}", "b".repeat(64)),
            config_digest: format!("sha256:{}", "c".repeat(64)),
            source_map_policy_digest: None,
            source_map_digest: None,
            compile_origin_tree_digests: Vec::new(),
            backend: "cargo-build-dir",
            events: Vec::new(),
            settled: false,
        }
    }

    #[test]
    fn ring_preserves_unsettled_records_and_evicts_settled_records_first() {
        let mut ring = ObservationRing::default();
        let mut large = run("old");
        let base = encoded_len(&large);
        large.project_component = "x".repeat(OBSERVATION_RING_BYTES - base - 100);
        assert!(ring.begin(large));

        assert!(!ring.begin(run("new-before-settle")));
        assert_eq!(ring.snapshot().len(), 1);
        assert_eq!(ring.snapshot()[0].run_id, "old");

        ring.settle("fixture", "old", "attempt-old");
        assert!(ring.begin(run("new-after-settle")));
        let retained = ring.snapshot();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].run_id, "new-after-settle");
    }

    #[test]
    fn ring_correlates_equal_run_ids_by_consumer() {
        let mut ring = ObservationRing::default();
        let mut first = run("shared-run");
        first.consumer = "consumer-a".to_owned();
        let mut second = run("shared-run");
        second.consumer = "consumer-b".to_owned();
        assert!(ring.begin(first));
        assert!(ring.begin(second));

        ring.event(
            "consumer-b",
            "shared-run",
            "attempt-shared-run",
            ObservationKind::Refused { reason: "busy" },
        );
        let records = ring.snapshot();
        assert_eq!(records.len(), 2);
        assert!(records[0].events.is_empty());
        assert_eq!(records[1].events.len(), 1);
    }
}
