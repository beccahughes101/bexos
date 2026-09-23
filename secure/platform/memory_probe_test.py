"""Run the firmware proof with a bounded child lifetime."""
import subprocess
import sys
import os
from pathlib import Path

diagnostic = Path(os.environ["TEST_UNDECLARED_OUTPUTS_DIR"]) / "qemu-exceptions.log"

result = subprocess.run([
    sys.argv[1], "-machine", "virt,secure=on,virtualization=on,gic-version=2",
    "-cpu", "max", "-m", "128", "-smp", "1", "-display", "none",
    "-monitor", "none", "-serial", "none", "-nodefaults", "-no-reboot",
    "-semihosting-config", "enable=on,target=native", "-bios", sys.argv[2],
    "-d", "int,guest_errors", "-D", str(diagnostic),
], capture_output=True, timeout=30)
output = result.stdout + result.stderr
sys.stdout.buffer.write(output)
if result.returncode or b"secure-platform: SEL2 and secure transition memory verified" not in output:
    print(diagnostic.read_text()[-16000:])
    raise SystemExit("secure platform execution/isolation proof failed")
