"""Exercise BL33 partition verification within the developer boot time budget.

Enter BL33 directly at EL2 to isolate verification from Trusty and kernel startup.
The full product tests cover authenticated firmware loading and secure services.
"""

import selectors
import shutil
import subprocess
import sys
import time


def main():
    verifier, kernel, bootfs, handoff, vbmeta = sys.argv[1:]
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        raise RuntimeError("qemu-system-aarch64 is required")
    command = [
        qemu, "-machine", "virt,virtualization=on", "-cpu", "cortex-a53",
        "-m", "1024", "-display", "none", "-serial", "stdio", "-monitor", "none",
        "-device", f"loader,file={verifier},cpu-num=0",
    ]
    for path, address in (
        (kernel, 0x40200000), (bootfs, 0x44000000),
        (handoff, 0x40100000), (vbmeta, 0x43E00000),
    ):
        command.extend(["-device", f"loader,file={path},addr={address},force-raw=on"])
    output = bytearray()
    started = time.monotonic()
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as child:
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ)
                while time.monotonic() - started < 15:
                    if selector.select(timeout=0.1):
                        chunk = child.stdout.read1(8192)
                        if not chunk:
                            break
                        output.extend(chunk)
                        if b"bl33: vbmeta and boot partitions verified" in output:
                            print(f"BL33 verification completed in {time.monotonic() - started:.2f}s")
                            return
                        if b"bl33: FATAL:" in output:
                            break
            raise RuntimeError("BL33 verification did not finish within 15s:\n" + output.decode(errors="replace"))
        finally:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


if __name__ == "__main__":
    main()
