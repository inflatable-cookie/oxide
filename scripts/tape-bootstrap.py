#!/usr/bin/env python3
"""Fixed child of the #240 supervisor: identify rustc then build debug Oxide."""
import os
from pathlib import Path
import subprocess
import sys

bin_dir = Path(os.environ['TAPE_TOOLCHAIN_BIN'])
rustc = bin_dir / 'rustc'
cargo = bin_dir / 'cargo'
result = subprocess.run([str(rustc), '--version', '--verbose'], check=True,
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
if not result.stdout.startswith('rustc 1.97.1 '):
    sys.exit('unexpected Rust toolchain')
print(result.stdout, end='', flush=True)
os.execv(str(cargo), [str(cargo), 'build', '--offline', '--locked', '--bin', 'oxide'])
