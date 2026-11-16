# Cooperative qualification acceptance target for future 309 work

Tom's decision `ea8ee47f-adc0-4cd3-b420-aaa30642d623` v2
(2026-11-16) adopts this reviewed contract as the acceptance target for future
309 work and scopes one independently reviewed Nucleus-owned execution-route
proposal, with no implementation or proof runs. Earlier decision `57374e05`
v2 authorized drafting and independent review only.

Adoption is prospective: it establishes no implemented or qualified route,
source acceptance, resource fit, backing capability, runtime readiness or proof
activation. Existing 309/061/304 briefs and their dispatch and merge holds remain
binding and require full prospective amendment before later activation. No old
receipt, native credit, source pin, budget or historical hard-clock or physical
claim migrates to this target. This changes no Barge guest/host or Rust setup
policy, consumer adoption or installation scope, or global proof rule.

## Work and accounting promises

One cumulative 600-second target starts at the **original first mandatory
effect**, including scheduler persistence, prepare, source capture/copies,
admission, workload, stops, wait attempts, finalization and delivery. No fresh
inner clock, later launch origin, added shutdown grace or assumed cross-clock
conversion is allowed. The prospective 560-second candidate, 530-second suite,
20-second stop and 2-second exit allocations must be explained from that same
origin, with prior work charged and overlapping allocations identified; they
are not additive allowances or OS guarantees.

Blocked syscalls, scheduling, fsync, positive waits/reaps and delivery may
outlast the target. Exhaustion stops qualification work through the supported
stop route and permanently prevents late PASS. It does not prove that stop
finished. Actual surviving parents retain unfinished child obligations;
storage/evidence owners retain accounting after timeout, report loss or turn
release. If an owner dies without a supported surviving actual-parent
relationship, custody stays unknown/FAILED/SPENT/retained. No perpetual parent,
portable wait-handle transfer or new cleanup authority is assumed.

**512 MiB is monitored owned scratch**, not an enforced aggregate physical
ceiling. The 384 MiB guard + 128 MiB outside decomposition is planning and
accounting *within* 512, neither enforced partitions nor free external space.
Inventory every actual root, capture, source, index, manifest, copy, log,
temporary publication, finalization and retained footprint across all providers,
including Nucleus DB/WAL/journal/results/output/evidence copies. Account
coexisting originals and copies and failure remnants. Retained evidence on
another backing remains charged after process closure.

Record measurement scope, device, method, units, freshness, missing coverage
and sampled high-water marks. Samples establish observations at those times;
logical caps remain logical. A scheduler reservation establishes only its named
capacity, not reserved disk space or an unobserved peak. External APFS metadata,
COW, rounding and physical allocation unknowns must be disclosed, never zeroed
or hidden inside a fabricated finite reserve. The total physical peak is
deliberately unguaranteed. That disclosure may coexist with acceptance only
when every prerequisite to safe execution is positively supported: unknown
ownership, writer state, required scratch coverage or reserve safety refuses
before effects and fails/holds after effects. An unknown peak is never labelled
bounded.

Missing, stale or unreadable required samples, accounting overflow, a known
512 MiB monitored breach or capture overflow refuse before effects; after
effects they fail, request supported stop and retain unresolved obligations.
The 22 GiB start/abort threshold (20 GiB reserve + 2 GiB margin) remains
cooperative: refuse unknown/below-threshold admission and stop/report detected
loss, without claiming a reservation or protection from other writers.

## Independent success obligations

Bind exact published head/tree, ordered inputs, selector, actual run,
participant, authority, effective executable/argv/cwd/environment and original
clock origin. Preserve genuine Effigy admission on the **actual selector** and
attributable Effigy completion alongside complete raw capture. No synthetic
hook/token/environment, unknown provider or maintenance/later-admission
substitution qualifies. There is no 8 MiB truncation exemption, uncharged
second log, or silent redaction/compression/truncation presented as complete.
If supported machinery cannot retain full capture, the unresolved capability
is complete, durably acknowledged, source-bound raw delivery within monitored
accounting; relaxing resource promises supplies no implementation evidence.

A later route must name its **actual surviving direct-parent handle graph**
before effects. For a directly execed actor the reported conditional graph is
invoker → Effigy owner → actor → actor-created custodian → monitor/leaves;
shells, supervisors and publishers add their actual creation edges. The
publisher's actual creator must retain its wait relationship too. This is an
obligation map, not an implemented surviving graph. Every required direct
parent must positively wait/reap its exact required direct children, including
actor, custodian, monitor, leaves and publisher. Identities, capacity holds,
EOF and empty-group samples transfer no handle. Never group-KILL the last
custodian. Owner loss cannot invent a controller, subreaper or successor.

Require separately and conjunctively:

- qualified, completed, custodian-bound **DECIDING** census and completed join;
- a separate **FINAL** certain-empty census for the exact closure group;
- separate ordinary provider evidence, where the proof owns an image, that
  the **entire owned backing and its residual allocations are absent**.

None replaces actual waits or another predicate. Directory deletion,
removed-container flags, empty groups and a detached or unmounted image do not
prove whole-backing absence. That absence never authorizes erasing evidence
retained on another backing; its storage remains accounted.

PASS requires every applicable source, **445-case**, oracle, admission,
capture, EOF, drain, successful fsync, binding, acknowledgement, publisher-wait,
deciding-join, final-census and backing obligation, with no missing, partial,
timed-out or late required evidence. Report time-budget outcome, workload
outcome, evidence completeness, measured resources and closure separately.
FAILED/SPENT/unknown and the original first fault remain permanent even after
later cleanup. Unknown or retained work blocks dependent occurrences and
lifecycle operations until separately supported reconciliation. No automatic
retry, refund, clock reset, fresh occurrence or cleanup follows.

## Written acceptance/refusal matrix

Pure written controls; no tests, fixtures or probes. **R** means refusal only
before *any* mandatory effect; **F** means FAILED/SPENT after effects; **U**
means unknown/SPENT after effects. F/U retain evidence, accounting and all
unproved custody/closure obligations; cleanup can later settle closure only.

| Observation | Expected status and retained obligation |
| --- | --- |
| Empty group without positive wait | U; actual parent still owes every missing wait. |
| Positive waits; lost/timed-out join | U/F; completed deciding join still required. |
| Missing/wrong DECIDING | U/F; qualified exact-custodian deciding evidence required. |
| Missing/nonempty FINAL | U/F; separate certain-empty final evidence required. |
| Normal workload exit; truncated/partial capture | F; complete raw capture and attributable completion still required. |
| Lost fsync/ack/publisher wait | U/F; successful durable delivery and actual publisher wait required. |
| Target hit; blocked I/O/live custodian | F, closure U; preserve surviving parent, stop/wait obligations and charged storage beyond target. |
| Unknown/stale scratch sample | R or U; no bounded-peak claim; restore supported safety knowledge before any dependent work. |
| Known scratch breach/overflow | R or F; stop through supported route, retain breach and all unresolved bytes. |
| External total physical peak unknown | Disclosed unguaranteed observation; alone no failure/PASS. R/U if a required safety input is unknown; all independent gates still apply. |
| Metadata/COW exceeds measured logical bytes | Disclose actual/unknown physical costs; no physical-ceiling claim. R/F for known monitored breach, R/U for unknown required safety input. |
| Missing image whole-backing absence evidence | U; provider must prove the entire owned backing and residual allocations absent. Keep backing, residuals and evidence held/accounted; detach, unmount, unlink and group facts do not suffice. |
| Report/turn/owner loss | U unless all required evidence was already durably accepted in target; prove surviving actual-parent custody or retain unknown. No replay/new occurrence. |
| Complete evidence only after target | F permanently; late evidence may reconcile closure, never acquire PASS or refund. |

## Source compatibility and gaps

Reported facts below are from the complete [Nucleus Spec 063](https://github.com/inflatable-cookie/nucleus/blob/a5a43e83a455aafa7a7882ba9afba2d7b0226a3f/docs/knowledge/specs/063-proof-execution-host-runs.md)
(docs-only PR 171, retained review `5923160295` at `a2ebe4c`, merged
`a5a43e8`; source audit base **N** = `5f443890784e7c289647ccf1ab9e1ea7fb42228e`),
and Effigy #1017's report (**E** = `dc6f5abf5a5c5e37658b7048e9f8cc644df48709`),
lane `v015-qualification` v19,
sha256 `368de92769966ffbc992f286e28324f7207c25e7573f8738fb82d20a8dd874b1`.
Both conclude `fits:false`; Effigy's Explore advice is not retained Queue
review. Installed `e6d7` source availability supplies no qualification credit.
Paths below are relative to the named repository/head. The target does not
establish a supported route; readiness remaining false is this contract's
inference.

| Predicate/promise | Existing supported source behavior; later gap |
| --- | --- |
| Exact source, admission, all 445 cases unchanged | Nucleus N `core/qa/proof-source.ts:21–59` binds published source; Effigy E `src/runner/task_engine/run/foreground.rs:181–225` supplies actual admission (reported). No proof-to-host-run integration selected; actual end-to-end binding remains required. |
| Resource promises changed only as above | Nucleus N `core/machine/process-exec.ts:252–262` starts its timer at spawn; Effigy E `src/runner/run_control.rs:95–147` prepares separately (reported). Later design must cover original effects and all monitored footprints; neither launch timers nor logical limits suffice. Hard blocked-I/O/physical guarantees are no longer proposed. |
| Actual parents and positive waits unchanged | Nucleus N `core/qa/supervisor-process.ts:46–55` returns PID, not a retained parent handle; Effigy E `crates/effigy-kernel/src/custody/spawn.rs:741–787` waits its own leader (reported). Surviving application graph/provider remains unsupported. |
| DECIDING/join, FINAL, whole-backing absence unchanged | Nucleus N `shared/schema/host-runs.ts:48–92` lacks these receipts (reported Spec 063). Separate qualified providers/evidence remain required, including absence of the entire owned backing and its residual allocations. |
| Complete capture/delivery unchanged | Nucleus N `core/host-runs/host-run-retained-output.ts:35–85` truncates/redacts and swallows sync faults; `core/machine/process-exec.ts:219–246` can settle without pipe closure. Complete durable raw delivery/ack/publisher-wait support remains unresolved. |
| Permanent failure/retention unchanged | Nucleus N `core/qa/proof-admission.ts:28–45`, `core/intake/proof-settlement.ts:45–62` preserve spent proofs (reported). No complete Oxide lifecycle/accounting implementation is established. |

## Disposition and premise coverage

The Nucleus route proposal, any separately scoped reviewed implementation and
accepted source and actual fit/readiness must precede a separately activated
adequate uncounted **445-case diagnostic**,
counted author, distinct retained reviewer and final exact-head disposition,
before 309 publication → 061 → 304. No ladder step is waived or activated.
Old receipts, grants, failures and clocks stay bound to their original bytes.
Unpublished/unvalidated local 309 source is ineligible. Ordinary guard `fcfda`,
parked 233 `c578`, candidate `4aba` and suite `b8d445` stay unchanged. All three
Barge and seven Oxide historical OPEN/UNCLOSED image resources remain untouched;
Rust 1.100 stays unqualified and 083 negative-only.

All nine [workflow premises](workflow.md#proof-authoring-premise-checklist)
are **not-executable-docs** here: (1) private HOME/rustup, (2) account-home
control, (3) environment isolation, (4) automatic adoption, (5) Cargo external
argv, (6) owned images, (7) compiler flags, (8) temporary storage and
(9) macOS mktemp. This task creates no executable cases; later execution must
apply them to actual bodies/helpers. Documentation validation is no native
evidence. Implementation readiness remains **false**.
