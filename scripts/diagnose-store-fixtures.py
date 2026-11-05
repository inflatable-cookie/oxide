#!/usr/bin/env python3
"""Uncounted, fixed three-case diagnostic; custody/bounds belong to the guard."""
import os
import sys
from pathlib import Path
import subprocess

cases = (
    "store::fresh_custody_tests::rows_reuse_one_parsed_state_and_reread_a_replaced_document",
    "store::fresh_custody_tests::a_disappeared_state_refuses",
    "store::tests::seed_crash_states_remain_untrusted_and_unrecorded_spawn_stays_held",
)
if sys.argv[1:] == ["--process-control"]:
    cases = ("store::tests::fixture_description_control",)
elif sys.argv[1:]:
    raise SystemExit("only --process-control is supported")
for index, case in enumerate(cases):
    log = Path(os.environ["OXIDE_PROOF_EVIDENCE"]) / f"store-fixture-{index}.log"
    with log.open("xb") as output:
        result = subprocess.run(["oxide", "test", "--bin", "oxide", "--locked", case,
                                 "--", "--exact", "--nocapture"], stdout=output, stderr=subprocess.STDOUT)
    print(f"{case}: exit={result.returncode} log={log}", flush=True)
print("uncounted diagnostic; qualified=false", flush=True)
raise SystemExit(1)
