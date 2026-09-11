"""Two QEMU boots retain the actual root disk; always reap each guest."""
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import tempfile
import time


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


signal.signal(signal.SIGTERM, interrupted)
with tempfile.TemporaryDirectory(prefix="bex-fw-") as directory:
    disk = Path(directory) / "firmware.raw"
    with disk.open("wb") as stream:
        stream.truncate(4096 + 4 * 65 * 1024 * 1024)
    for boot in (1, 2):
        child = subprocess.Popen([
            "qemu-system-x86_64", "-machine", "q35,accel=tcg", "-cpu", "max",
            "-m", "1024", "-display", "none", "-monitor", "none", "-serial", "stdio",
            "-no-reboot", "-net", "none", "-kernel", sys.argv[1],
            "-drive", f"if=none,id=firmware,format=raw,cache=writeback,file={disk}",
            "-device", "isa-ide,id=firmwarebus,iobase=0x1f0,iobase2=0x3f6,irq=14",
            "-device", "ide-hd,bus=firmwarebus.0,unit=0,drive=firmware",
            "-fw_cfg", f"name=opt/bexos/firmware-probe-boot,string={boot}",
        ], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        output = bytearray()
        try:
            deadline = time.monotonic() + 30
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ)
                while b"svm-probe: independent guest register contexts verified" not in output:
                    if time.monotonic() >= deadline or child.poll() is not None or b"FAILED" in output:
                        raise RuntimeError(f"firmware disk boot {boot} failed")
                    for key, _ in selector.select(.1):
                        output.extend(key.fileobj.read1(4096))
                        if len(output) > 1024 * 1024:
                            raise RuntimeError("probe output overflow")
            if b"root firmware disk durable across owner reconstruction and reboot" not in output:
                raise RuntimeError("missing actual controller durability evidence")
        finally:
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
            sys.stdout.buffer.write(output)
