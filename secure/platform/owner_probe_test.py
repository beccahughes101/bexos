"""Run the secure-EL2 owner and two-bank isolation proof."""
import os
from pathlib import Path
import subprocess
import sys

diagnostic = Path(os.environ["TEST_UNDECLARED_OUTPUTS_DIR"]) / "qemu-owner.log"
result = subprocess.run([
    sys.argv[1], "-machine", "virt,secure=on,virtualization=on,gic-version=2",
    "-cpu", "max", "-m", "128", "-smp", "1", "-display", "none",
    "-monitor", "none", "-serial", "none", "-nodefaults", "-no-reboot",
    "-semihosting-config", "enable=on,target=native", "-bios", sys.argv[2],
    "-d", "int,guest_errors", "-D", str(diagnostic),
], capture_output=True, timeout=30)
output = result.stdout + result.stderr
sys.stdout.buffer.write(output)
marker = b"secure-owner: S-EL1 bank replacement and peer isolation verified"
if result.returncode or marker not in output or b"FAIL" in output:
    print(diagnostic.read_text()[-16000:])
    raise SystemExit("secure execution-owner proof failed")

