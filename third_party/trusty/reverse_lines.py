#!/usr/bin/env python3
"""The line-reversal operation used by Trusty's make dependency ordering."""
from pathlib import Path
import sys

for filename in sys.argv[1:] or ["-"]:
    data = sys.stdin.buffer.read() if filename == "-" else Path(filename).read_bytes()
    sys.stdout.buffer.writelines(reversed(data.splitlines(keepends=True)))
