#!/usr/bin/env python3
"""The workspace unit/integration suite for the Barge adoption (oxide#314).

Ordinary process handling: one fresh run directory inside the task's own
`TMPDIR` (which the task runner places on the selected volume), everything
inside it, removed only after the run. The suite is single-threaded because
several fixtures take the same account's admission lock and Barge hold locks, so
parallel test threads would observe each other's fixtures.

The internal-volume fixtures are skipped: their premise is a private root on
the boot Data volume, which this run may not use. They remain covered by the
repository's separate host/QA route, and their names are listed so the skip is
auditable.
"""

import os
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Fixtures whose premise is a private root on the internal boot Data volume (or
# a scratch root that must live off /Volumes). This run's rules forbid using
# the internal drive, so they are skipped by name rather than silently.
INTERNAL_VOLUME_TESTS = [
    "store::scratch_internal_tests::",
    "scratch_admission::tests::",
    "scratch_io::tests::",
    "prune::tests::",
    "volume::tests::a_caller_owned_private_root_passes_and_a_writable_one_refuses",
    "config::tests::git_work_trees_and_aliases_refuse",
    # The CLI integration fixtures create an "internal" (off-/Volumes) pool to
    # prove the external-pool refusal and init/help routing; that premise needs
    # the internal drive this run may not use.
    "a_pool_inside_the_task_scratch_root_is_still_created",
    "an_explicit_pool_outside_the_task_scratch_root_refuses",
    "help_is_cargos_help_without_a_slot_or_pool",
    "init_and_new_help_skip_the_handoff_check",
    "init_readiness_names_the_selected_pool",
    "outputs_plan_reports_the_whole_target_migration",
]


def task_scratch() -> str:
    """The task's own TMPDIR, which the runner places on the selected volume."""
    base = os.environ.get("TMPDIR") or ""
    if not base or not os.path.isabs(base):
        raise SystemExit("TMPDIR must name the task's absolute scratch directory")
    return base


def main() -> int:
    parent = task_scratch()
    run = tempfile.mkdtemp(prefix="run.", dir=parent)
    environment = dict(os.environ)
    environment.update({"TMPDIR": run, "TMP": run, "TEMP": run})
    # No internal root: these fixtures are skipped by name below.
    for name in ("OXIDE_TEST_INTERNAL_ROOT", "OXIDE_TASK_SCRATCH_ROOT"):
        environment.pop(name, None)
    command = [
        "oxide",
        "test",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "--test-threads=1",
    ]
    for name in INTERNAL_VOLUME_TESTS:
        command += ["--skip", name]
    print("barge-adoption: scratch", run, flush=True)
    print(
        "barge-adoption: skipped internal-volume fixtures:",
        ", ".join(INTERNAL_VOLUME_TESTS),
        flush=True,
    )
    try:
        status = subprocess.call(command, cwd=ROOT, env=environment)
    finally:
        # Only this run's own directory, created above.
        shutil.rmtree(run, ignore_errors=True)
    print("barge-adoption: exit", status, flush=True)
    return status


if __name__ == "__main__":
    sys.exit(main())
