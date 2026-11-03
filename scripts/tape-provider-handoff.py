#!/usr/bin/env python3
"""Fail closed until Queue supplies the qualified shared-provider route.

The capture task does not issue, validate, or consume its own admission
packet. This entry performs no preparation and launches no child.
"""
import sys


def handoff():
    raise SystemExit(
        'oxide#240: the planner must bind a qualified shared private-build '
        'validation packet and exact-head execution route; no local launch'
    )


if __name__ == '__main__':
    handoff()
