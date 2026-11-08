#!/usr/bin/env python3
"""Metadata-only host control for the observation volume resolver (oxide#292).

Runs ONLY as the whole command of the qualified shared supervisor (60 s limit,
32 MiB monitored scratch). It creates one fresh empty directory inside the guard's
own scratch root, resolves that nested directory's volume with the real corrected
helper (`tape-embedded-observation.resolve_volume` over the real `Seams`), and
re-derives the mount, device, device-node and selected-UUID facts independently.

It runs no engine, staging build, seed, image, mount, registration, payload read
or observation. The one new filesystem input is the nested directory, which the
guard owns and removes; this script removes nothing. Its result is a metadata
premise control: it is not an engine, qualification or observation credit.

Usage (under the guard only):
    tape-observation-volume-control.py
"""
from __future__ import annotations

import importlib.util
import json
import os
import secrets
import sys

sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
SCHEMA = "oxide.capture.volume-control/1"
EVIDENCE_NAME = "volume-control.json"
CLAIM = ("metadata premise only: the corrected volume resolver's facts for one "
         "nested owned directory; no engine, observation, staging, image or "
         "registration credit, and qualified is false")


def load_driver():
    """The observation driver as a module. Importing it runs no effect: its
    top level only defines constants, classes and functions."""
    spec = importlib.util.spec_from_file_location(
        "tape_embedded_observation", os.path.join(HERE, "tape-embedded-observation.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def publish(evidence, document):
    """One bounded document in the guard's evidence directory, created exclusively."""
    path = os.path.join(evidence, EVIDENCE_NAME)
    with open(path, "x") as handle:
        json.dump(document, handle, indent=2, sort_keys=True)
        handle.write("\n")
    return path


def control(driver, environ):
    scratch = environ["OXIDE_TASK_SCRATCH_ROOT"]
    evidence = environ["OXIDE_PROOF_EVIDENCE"]
    seams = driver.Seams()
    guard = driver.guard_context(seams, environ)
    nested = os.path.join(scratch, "volume-control-" + secrets.token_hex(4))
    os.mkdir(nested, 0o700)
    document = {"schema": SCHEMA, "qualified": False, "claim": CLAIM,
                "nested": nested, "scratch_root": scratch,
                "guard": {"supervisor_sha256": guard["supervisor_sha256"],
                          "run_id": guard["run_id"]}}
    try:
        facts = driver.resolve_volume(nested, seams)
        problems = []
        mount = facts["mount"]
        if facts["uuid"] != driver.SELECTED_VOLUME_UUID:
            problems.append(f"the volume UUID is {facts['uuid']}, not the selected Stash UUID")
        if facts["boot"]:
            problems.append("the mount is the boot volume")
        if not os.path.ismount(mount):
            problems.append(f"{mount} is not a kernel mount point")
        if os.path.commonpath([mount, nested]) != mount or mount == nested:
            problems.append(f"{mount} is not a strict ancestor of the nested directory")
        if os.path.realpath(mount) != mount:
            problems.append(f"the mount {mount} is not canonical")
        if seams.device(nested) != facts["device"] or seams.device(mount) != facts["device"]:
            problems.append("the nested directory and its mount are not on one device")
        if seams.device_node(facts["node"]) != facts["device"]:
            problems.append(f"the device node {facts['node']} is not the mount's device")
        document.update({
            "state": "pass" if not problems else "fail",
            "mount": mount, "device": facts["device"], "device_node": facts["node"],
            "volume_uuid": facts["uuid"], "steps": facts["steps"],
            "problems": problems,
        })
    except driver.Obstruction as error:
        document.update({"state": "refused", "reason": str(error), "problems": []})
    published = publish(evidence, document)
    print(f"volume-control: {document['state']} {published} "
          f"(mount {document.get('mount')}, uuid {document.get('volume_uuid')}); "
          "qualified=false")
    if document["state"] == "pass":
        return 0
    if document["state"] == "fail":
        for problem in document["problems"]:
            print(f"volume-control: {problem}", file=sys.stderr)
    return 1


def main():
    environ = dict(os.environ)
    required = ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_EVIDENCE",
                "OXIDE_PROOF_RUN_ID", "OXIDE_PROOF_DEADLINE_NS")
    if not all(environ.get(name) for name in required):
        print("volume-control: refused: this control runs only under the qualified "
              "shared supervisor", file=sys.stderr)
        return 2
    driver = load_driver()
    try:
        return control(driver, environ)
    except driver.Obstruction as error:
        print(f"volume-control: refused: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
