"""Bounded Q35 development boot; never invokes a secure-world fixture."""
import os
import socket
import selectors
import subprocess
import struct
import tempfile
import sys
import time

image, bootfs, handoff, evidence, disk = sys.argv[1:]
scratch = tempfile.TemporaryDirectory(prefix='bex-q35-', dir='/tmp')
debug_socket = os.path.join(scratch.name, 'd.sock')
trace = os.path.join(os.environ.get('TEST_UNDECLARED_OUTPUTS_DIR', '/tmp'), 'q35.trace')
command = ['qemu-system-x86_64', '-machine', 'q35,accel=tcg', '-cpu', 'max', '-smp', '4', '-m', '1024', '-display', 'none', '-monitor', 'none', '-serial', 'stdio', '-no-reboot', '-d', 'int,guest_errors', '-D', trace, '-kernel', image,
           '-drive', f'if=none,id=nvme0,file={disk},format=raw,snapshot=on', '-device', 'nvme,drive=nvme0,serial=bexos-nvme0',
           '-netdev', 'user,id=net0', '-device', 'virtio-net-pci,disable-legacy=on,netdev=net0,mac=52:54:00:12:34:56',
           '-device', 'virtio-serial-pci,disable-legacy=on', '-chardev', f'socket,id=console,path={debug_socket},server=on,wait=off', '-device', 'virtserialport,chardev=console,nr=1,name=debug0']
with open(handoff, 'rb') as descriptor:
    bootfs_address = hex(struct.unpack_from('<Q', descriptor.read(), 4 * 8)[0])
for path, address in [(bootfs, bootfs_address), (handoff, '0x01000000'), (evidence, '0x07f00000')]:
    command += ['-device', f'loader,file={path},addr={address},force-raw=on']
child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
debug = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
output = bytearray()
panic_deadline = None
try:
    for attempt in range(100):
        try:
            debug.connect(debug_socket)
            break
        except (FileNotFoundError, ConnectionRefusedError):
            time.sleep(.02)
    with selectors.DefaultSelector() as selector:
        selector.register(child.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + 120
        marker = b'appd: guest persistence and disk-only application verified'
        while marker not in output:
            if (b"kernel exception" in output or b": panic:" in output or b"appd: boot failed" in output) and panic_deadline is None:
                panic_deadline = time.monotonic() + 1
            if (panic_deadline is not None and time.monotonic() >= panic_deadline) or time.monotonic() >= deadline or child.poll() is not None:
                raise AssertionError('Q35 development boot failed:\n' + output.decode(errors='replace'))
            for key, _ in selector.select(.1):
                chunk = key.fileobj.read1(65536)
                output.extend(chunk)
                print(chunk.decode(errors="replace"), end="", flush=True)
    print(output.decode(errors='replace'))
finally:
    debug.close()
    child.terminate()
    try:
        child.wait(timeout=3)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()

    scratch.cleanup()
