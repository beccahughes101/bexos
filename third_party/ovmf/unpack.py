"""Extract the hash-pinned QEMU OVMF firmware pair into declared Bazel outputs."""
import bz2
from pathlib import Path
import sys

if len(sys.argv) != 5:
    raise SystemExit('usage: unpack CODE.bz2 CODE.fd VARS.bz2 VARS.fd')
code = bz2.decompress(Path(sys.argv[1]).read_bytes())
variables = bz2.decompress(Path(sys.argv[3]).read_bytes())
if len(code) + len(variables) != 4 * 1024 * 1024:
    raise SystemExit('unexpected OVMF flash layout')
Path(sys.argv[2]).write_bytes(code)
Path(sys.argv[4]).write_bytes(variables)
