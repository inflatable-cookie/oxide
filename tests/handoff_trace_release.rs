#![cfg(not(debug_assertions))]

// The genuine release-module counterfactual: this integration target imports
// the actual `handoff_trace` source. Its in-module release tests carry the
// qualified supervisor's two independent identities — the guard receipt's
// session `run_id` and the marker's own `run_id` bound through
// `scratch_marker.run_id` — so a release child under the full private layout
// must still write nothing.

#[path = "../src/bin/oxide/handoff_trace.rs"]
mod handoff_trace;
