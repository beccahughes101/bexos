"""Exercise real SVM guest entry and an NPT fault; always reap the emulator."""
import selectors
import signal
import subprocess
import sys
import time


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


signal.signal(signal.SIGTERM, interrupted)
dma = "--dma" in sys.argv
child = subprocess.Popen([
    'qemu-system-x86_64', '-machine', 'q35,accel=tcg', '-cpu', 'max',
    '-m', '1024', '-display', 'none', '-monitor', 'none', '-serial', 'stdio',
    '-no-reboot', '-kernel', sys.argv[1],
    *(['-device', 'intel-iommu,intremap=on,eim=off', '-device', 'edu,addr=5,dma_mask=0xffffffffffffffff'] if dma else []),
], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
output = bytearray()
try:
    deadline = time.monotonic() + 30
    with selectors.DefaultSelector() as selector:
        selector.register(child.stdout, selectors.EVENT_READ)
        while b'svm-probe: independent guest register contexts verified' not in output:
            if time.monotonic() >= deadline or child.poll() is not None or b'FAILED' in output:
                raise RuntimeError('SVM execution/isolation probe failed')
            for key, _ in selector.select(.1):
                chunk = key.fileobj.read1(4096)
                output.extend(chunk)
                if len(output) > 1024 * 1024:
                    raise RuntimeError('SVM probe log overflow')
    if b'svm-probe: guest hypercall verified' not in output:
        raise RuntimeError('Missing hypercall evidence')
    for marker in (b'domain read-only and execute-disable enforced', b'revoked domain page access rejected', b'uncooperative guest preempted and resumed'):
        if marker not in output:
            raise RuntimeError('Missing page permission rejection evidence')
    if b'svm-probe: nested-page isolation verified' not in output:
        raise RuntimeError('Missing NPT rejection evidence')
    if b'svm-probe: virtual interrupt masking and delivery verified' not in output:
        raise RuntimeError('Missing virtual interrupt delivery evidence')
    if b'svm-probe: pending virtual interrupt delivered after protected CPU restore' not in output:
        raise RuntimeError('Missing pending virtual interrupt restore evidence')
    if b'svm-probe: interrupt fabric restored without duplicate delivery' not in output:
        raise RuntimeError('Missing restored interrupt-controller delivery evidence')
    if b'svm-probe: protected and SIPI guest entry verified' not in output:
        raise RuntimeError('Missing BSP/AP entry-mode evidence')
    if b'svm-probe: device MSR and nested virtualization intercepted' not in output:
        raise RuntimeError('Missing privileged instruction rejection evidence')
    if b'svm-probe: hypercall capability ownership and stale-handle rejection verified' not in output:
        raise RuntimeError('Missing monitor transport rejection evidence')
    if b'svm-probe: protected CPU records resumed interleaved guest execution' not in output:
        raise RuntimeError('Missing actual guest resume after CPU record restore')
    if dma and b'svm-probe: monitor and secure-domain DMA writes explicitly rejected' not in output:
        raise RuntimeError('Missing hardware DMA rejection evidence')
finally:
    if child.poll() is None:
        child.terminate()
    try:
        child.wait(timeout=3)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()
    sys.stdout.buffer.write(output)
