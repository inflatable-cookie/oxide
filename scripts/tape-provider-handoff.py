#!/usr/bin/env python3
"""Thin entry for the shared candidate consumer; no local admission policy."""
import sys

sys.dont_write_bytecode = True
import private_build_candidate as consumer


def handoff(argv=None):
    args = list(sys.argv[1:] if argv is None else argv)
    if len(args) != 4 or args[0] != '--input' or args[2] != '--sha256':
        raise SystemExit('oxide#240: missing planner-bound candidate input')
    # Guard validation precedes reading any caller-supplied packet.
    consumer.provider.supervisor_context()
    options = consumer._document(args[1], args[3])
    return consumer.consume(options)


if __name__ == '__main__':
    try:
        sys.exit(handoff())
    except (consumer.provider.GateRefusal, KeyError, ValueError, TypeError, OSError) as error:
        sys.exit(f'oxide#240 candidate handoff refused: {error}')
