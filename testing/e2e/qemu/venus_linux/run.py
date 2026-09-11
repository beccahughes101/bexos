"""Boot one disposable Linux VM; always reap QEMU, including failed boots."""
import os
from pathlib import Path
import platform
import subprocess
import selectors
import time
import sys
import signal

# Bazel timeout/cancellation must unwind the same child cleanup as failures.
signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))

kernel, initramfs = map(str, map(Path.resolve, map(Path, sys.argv[1:3])))
accel = "hvf" if platform.system() == "Darwin" and platform.machine() == "arm64" else "tcg"
command = ["qemu-system-aarch64", "-machine", "virt", "-accel", accel,
           "-cpu", "host" if accel == "hvf" else "max", "-m", "3072", "-smp", "2",
           "-kernel", kernel, "-initrd", initramfs, "-append", "console=ttyAMA0 rdinit=/init panic=-1",
           "-display", "none", "-monitor", "none", "-serial", "stdio", "-nic", "none", "-no-reboot"]
if len(sys.argv) == 4:
    disk = str(Path(sys.argv[3]).resolve())
    command += ["-drive", "if=none,id=guestdisk,file=" + disk + ",format=raw,snapshot=on",
                "-device", "virtio-blk-pci,drive=guestdisk"]
workload = int(os.environ.get("SCENED_WORKLOAD", "0"))
assert 0 <= workload <= 6
if workload:
    command[command.index("-append")+1] += f" scened_workload={workload}"
process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
output = bytearray()
try:
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        # Full offscreen renderer restart checks precede normal guest boot.
        # Bound the complete software fixture independently of guest deadlines.
        # This includes boot, hotplug, seven package preparations, and rendering.
        # The guest Bazel target uses the larger timeout class so this wrapper
        # owns failure cleanup instead of being killed at the 900-second mark.
        fixture_seconds = (1140 if len(sys.argv) == 4 else 840) + (((32+256+1)*30+120) * (5 if workload == 6 else 1) if workload else 0)
        deadline = time.monotonic() + fixture_seconds
        while True:
            if time.monotonic() >= deadline:
                raise TimeoutError(f"Linux fixture exceeded {fixture_seconds} seconds")
            if not selector.select(timeout=1):
                continue
            chunk = os.read(process.stdout.fileno(), 16384)
            if not chunk:
                break
            output.extend(chunk)
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
    process.wait(timeout=5)
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
path = Path(os.environ.get("TEST_UNDECLARED_OUTPUTS_DIR", "/tmp")) / "venus-linux.log"
path.write_bytes(output)
assert process.returncode == 0, process.returncode
assert b"VENUS_LINUX_HOST_VERIFIED" in output and b"VENUS_LINUX_FIXTURE_EXIT=0" in output
if len(sys.argv) == 4:
    assert b"BEXOS_VENUS_DEVICE_CPU_SCANOUT_VERIFIED" in output
    assert b"BEXOS_VENUS_INPUT_TRANSPLANT_VERIFIED" in output

if workload:
    assert b"SCENED_SUSTAINED_VERIFIED" in output
