"""Supervise an Effigy-owned test process group; never leave a hung test alive."""
import os
import signal
import subprocess
import sys

process = subprocess.Popen(sys.argv[1:], start_new_session=True)
try:
    code = process.wait(timeout=300)
except subprocess.TimeoutExpired:
    # This is exactly the group created above, never a process-name match.
    # TERM first: `oxide` runs Cargo in its own group and forwards TERM to it.
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()
    print("bounded repository test exceeded 300 seconds", file=sys.stderr)
    code = 124
sys.exit(code)
