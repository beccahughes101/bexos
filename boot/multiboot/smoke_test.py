import selectors
import subprocess
import sys
import time

for image in sys.argv[1:]:
    child = subprocess.Popen(['qemu-system-x86_64', '-machine', 'q35,accel=tcg', '-cpu', 'max', '-m', '1024', '-display', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot', '-kernel', image], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
    output = bytearray()
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(child.stdout, selectors.EVENT_READ)
            deadline = time.monotonic()+30
            while b'bexos-multiboot: handoff verified' not in output:
                if time.monotonic() >= deadline or child.poll() is not None:
                    raise AssertionError(f'{image}: handoff failed: {output.decode(errors="replace")}')
                for key, _ in selector.select(.1):
                    output.extend(key.fileobj.read1(4096))
        print(f'{image}: Multiboot handoff passed')
    finally:
        child.terminate()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
