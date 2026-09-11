"""Verify firmware authentication and explicit signature rejection in QEMU."""
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ACCEPTED = b'efi-probe: firmware authenticated image; SecureBoot=1 SetupMode=0'
DISABLED = b'efi-probe: secure boot disabled; refusing execution'
ENTERED = b'efi-probe: entered image'
REJECTED = b': Access Denied\r\n'
UNPROTECTED = b'efi-probe: unprotected variable flash; refusing execution'


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


def boot(code, variables, payload, expected, label, flash_protected=True, cpu='max',
         extra_args=(), additional_markers=(), timeout=30, on_output=None, memory_mib=1024, forbidden_markers=(), persistent_variables=None):
    with tempfile.TemporaryDirectory(prefix='bexos-efi-') as temporary:
        work = Path(temporary)
        efi = work / 'esp' / 'EFI' / 'BOOT'
        efi.mkdir(parents=True)
        (efi / 'BOOTX64.EFI').write_bytes(payload)
        variable_store = Path(persistent_variables) if persistent_variables else work / 'vars.fd'
        if not variable_store.exists():
            shutil.copyfile(variables, variable_store)
        child = subprocess.Popen([
            'qemu-system-x86_64', '-machine', 'q35,accel=tcg,smm=on',
            '-cpu', cpu, '-m', str(memory_mib), '-display', 'none', '-monitor', 'none',
            '-serial', 'stdio', '-no-reboot', '-net', 'none',
            '-global', 'driver=cfi.pflash01,property=secure,value=' + ('on' if flash_protected else 'off'),
            '-drive', f'if=pflash,format=raw,unit=0,readonly=on,file={code}',
            '-drive', f'if=pflash,format=raw,unit=1,file={variable_store}',
            '-drive', f'format=raw,file=fat:rw:{work / "esp"}',
            *extra_args,
        ], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        output = bytearray()
        try:
            deadline = time.monotonic() + timeout
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ)
                while expected not in output or any(marker not in output for marker in additional_markers):
                    if b'monitor-runtime: firmware recovery required' in output and b'firmware recovery required' not in expected:
                        raise RuntimeError(f'{label}: unexpected authenticated firmware recovery requirement')
                    if expected not in output and any(marker in output for marker in (
                            b'refusing execution', b'boot payload snapshot rejected',
                            b'boot payload authentication rejected',
                            b'stale generation rejected by Trusty RPMB floor',
                            b'unlocked Trusty boot state rejected', b'invalid boot generation rejected')):
                        raise RuntimeError(f'{label}: unexpected explicit boot rejection')
                    if any(marker in output for marker in (b'FAILED', b'monitor-runtime: root fault frame', b'monitor-runtime: rejected', b'appd: boot failed:', b'wasm_runner: failed:', b'kernel: panic:', b'panicked at', b'panic (caller ')):
                        raise RuntimeError(f'{label}: guest verification failed')
                    if expected in (REJECTED, DISABLED, UNPROTECTED) and ACCEPTED in output:
                        raise RuntimeError(f'{label}: rejected configuration was accepted')
                    if expected == REJECTED and ENTERED in output:
                        raise RuntimeError(f'{label}: untrusted image executed')
                    if child.poll() is not None:
                        raise RuntimeError(f'{label}: QEMU exited ({child.returncode}) before explicit firmware evidence')
                    if time.monotonic() >= deadline:
                        raise RuntimeError(f'{label}: timed out before explicit firmware evidence')
                    for key, _ in selector.select(.1):
                        chunk = key.fileobj.read1(4096)
                        output.extend(chunk)
                        sys.stdout.buffer.write(chunk)
                        sys.stdout.flush()
                        if on_output is not None:
                            on_output(output)
                        if len(output) > 1024 * 1024:
                            raise RuntimeError('firmware output limit exceeded')
            if expected == REJECTED and ENTERED in output:
                raise RuntimeError(f'{label}: rejected image executed')
            if expected == REJECTED and b'BdsDxe: failed to load Boot0001' not in output:
                raise RuntimeError(f'{label}: missing firmware LoadImage rejection')
            if b'FAILED' in output or b'appd: boot failed:' in output or b'wasm_runner: failed:' in output or b'monitor-runtime: rejected' in output:
                raise RuntimeError(f'{label}: guest failed after reporting evidence')
            if any(marker not in output for marker in additional_markers):
                raise RuntimeError(f'{label}: missing authenticated execution chain evidence')
            if any(marker in output for marker in forbidden_markers):
                raise RuntimeError(f'{label}: rejected boot entered a forbidden execution stage')
            print(f'{label}: verified', flush=True)
        finally:
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
            sys.stdout.buffer.write(child.stdout.read(1024 * 1024))
            sys.stdout.flush()


def main():
    signal.signal(signal.SIGTERM, interrupted)
    code, enrolled, revoked, signed, unsigned, empty, wrong_key = map(Path, sys.argv[1:])
    payload = signed.read_bytes()
    boot(code, enrolled, payload, ACCEPTED, 'signed')
    boot(code, enrolled, unsigned.read_bytes(), REJECTED, 'unsigned')
    boot(code, enrolled, wrong_key.read_bytes(), REJECTED, 'wrong key')
    boot(code, revoked, payload, REJECTED, 'revoked')
    boot(code, empty, payload, DISABLED, 'setup mode')
    boot(code, enrolled, payload, UNPROTECTED, 'unprotected flash', flash_protected=False)
    tampered = bytearray(payload)
    # Modify code while retaining a loadable PE header and the original signature.
    pe = int.from_bytes(tampered[0x3c:0x40], 'little')
    optional_size = int.from_bytes(tampered[pe+20:pe+22], 'little')
    section = pe + 24 + optional_size
    raw = int.from_bytes(tampered[section+20:section+24], 'little')
    tampered[raw] ^= 1
    boot(code, enrolled, tampered, REJECTED, 'modified code')


if __name__ == '__main__':
    main()
