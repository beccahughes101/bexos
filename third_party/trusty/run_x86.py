#!/usr/bin/env python3
"""Own a standalone Q35 Trusty instance and its persistent development RPMB."""
import argparse
import os
from pathlib import Path
import selectors
import shutil
import signal
import stat
import subprocess
import tempfile
import time


def stop(child):
    if child is None:
        return
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.kill()
    child.wait()


def boot(image, rpmbd, state, work, markers, timeout, memory_mib=1024, disk=None, firmware=None, recovery_cut=None):
    socket = work / 'rpmb.sock'
    socket.unlink(missing_ok=True)
    proxy = qemu = debug_client = None
    debug = work / "debug.sock"
    debug.unlink(missing_ok=True)
    try:
        proxy = subprocess.Popen([str(rpmbd), '--dev', str(state), '--sock', str(socket)], stdin=subprocess.DEVNULL)
        deadline = time.monotonic()+10
        while not socket.exists() or not stat.S_ISSOCK(socket.stat().st_mode):
            if proxy.poll() is not None:
                raise RuntimeError('RPMB proxy exited before readiness')
            if time.monotonic() >= deadline:
                raise TimeoutError('RPMB proxy readiness timed out')
            time.sleep(.01)
        command = ['qemu-system-x86_64', '-machine', 'q35,accel=tcg', '-cpu', 'max', '-smp', '1', '-m', str(memory_mib),
                   '-display', 'none', '-monitor', 'none', '-no-reboot', '-kernel', str(image),
                   '-serial', 'stdio', '-serial', f'unix:{socket}', '-d', 'guest_errors']
        if os.environ.get('BEXOS_QEMU_DIAGNOSTICS') == '1':
            command += ['-d', 'guest_errors,cpu_reset,int']
        if disk is not None:
            import x86_devices
            command += x86_devices.arguments(disk, debug)
        if firmware is not None:
            command += ['-drive', f'if=none,id=firmwaredisk,format=raw,cache=writeback,file={firmware}',
                        '-device', 'isa-ide,id=firmwarebus,iobase=0x1f0,iobase2=0x3f6,irq=14',
                        '-device', 'ide-hd,bus=firmwarebus.0,unit=0,drive=firmwaredisk']
        if recovery_cut is not None:
            command += ['-fw_cfg', f'name=opt/bexos/recovery-interrupt,string={recovery_cut}']
        qemu = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        if disk is not None:
            debug_client = x86_devices.connect(debug, qemu)
        output = bytearray()
        failure_deadline = None
        deadline = time.monotonic()+timeout
        with selectors.DefaultSelector() as selector:
            selector.register(qemu.stdout, selectors.EVENT_READ)
            while True:
                if failure_deadline is not None and time.monotonic() >= failure_deadline:
                    raise RuntimeError("Trusty boot failed; see guest diagnostics above")
                if time.monotonic() >= deadline:
                    raise TimeoutError('Trusty boot/acceptance deadline exceeded')
                for key, _ in selector.select(.2):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        raise RuntimeError(f'QEMU exited before completion: {qemu.poll()}')
                    os.write(1, chunk)
                    output.extend(chunk)
                    if len(output) > 16*1024*1024:
                        raise RuntimeError('Trusty output limit exceeded')
                    if any(marker in output for marker in (b'panic (caller ', b'panicked at', b'appd: boot failed:', b'wasm_runner: failed:', b'bexos-loader: invalid', b'monitor-runtime: rejected', b'monitor-runtime: FAILED', b'monitor-runtime: root fault frame')):
                        if failure_deadline is None:
                            failure_deadline = time.monotonic() + 1
                    if failure_deadline is None and markers and all(marker.encode() in output for marker in markers):
                        return bytes(output)
    finally:
        stop(qemu)
        if debug_client is not None:
            debug_client.close()
        stop(proxy)
        socket.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('image', type=Path)
    parser.add_argument('rpmbd', type=Path)
    parser.add_argument('template', type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--acceptance', action='store_true')
    mode.add_argument('--smoke', action='store_true')
    mode.add_argument('--boot-selection', action='store_true')
    mode.add_argument('--firmware-recovery', action='store_true')
    mode.add_argument('--trusty-recovery', action='store_true')
    parser.add_argument('--timeout', type=int, default=600)
    parser.add_argument('--memory-mib', type=int, choices=[1024, 2048, 3072], default=1024)
    parser.add_argument('--disk', type=Path)
    parser.add_argument('--marker', action='append', default=[])
    parser.add_argument('--nucleus', action='store_true')
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    image, rpmbd, template = (p.resolve(strict=True) for p in [args.image, args.rpmbd, args.template])
    with tempfile.TemporaryDirectory(prefix='bexos-trusty-x86-') as temporary:
        work = Path(temporary)
        state = Path(os.environ.get('BEXOS_QEMU_RPMB_STATE', work/'RPMB_DATA'))
        if not state.exists():
            state.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(template, state)
            state.chmod(0o600)
        disk = work / 'normal.img' if args.disk else None
        if disk is not None:
            shutil.copyfile(args.disk.resolve(strict=True), disk)
            disk.chmod(0o600)
        if args.boot_selection:
            for marker in (
                'monitor-runtime: protected boot trial persisted without advancing commitment',
                'monitor-runtime: protected boot commitment survives retries and rejects stale rollback',
                'monitor-runtime: protected committed selection recovered after reboot',
            ):
                boot(image, rpmbd, state, work, [marker], args.timeout, args.memory_mib, disk)
            return
        if args.firmware_recovery or args.trusty_recovery:
            import firmware_recovery_test
            firmware_recovery_test.run(args, image, rpmbd, template, state, work, disk, boot)
            return
        firmware = None
        if args.nucleus:
            firmware = work / 'firmware.raw'
            with firmware.open('xb') as output:
                output.truncate(4096 + 4 * 65 * 1024 * 1024)
                output.flush()
                os.fsync(output.fileno())
            args.marker.append('monitor-runtime: nucleus repeated signed replacement and post-progress fault and hang recovery verified')
        markers = ['bexos-trusty-x86: ready', 'bexos-trusty-x86: RPMB exchange complete'] if args.smoke else []
        if args.acceptance:
            markers = ['bexos-authmgr-acceptance: complete', 'bexos-security-acceptance: complete', 'bexos-security-acceptance: storage generation=1']
        markers.extend(args.marker)
        boot(image, rpmbd, state, work, markers, args.timeout if markers else 365*24*3600, args.memory_mib, disk, firmware)
        if args.acceptance:
            markers[2] = 'bexos-security-acceptance: storage generation=2'
            boot(image, rpmbd, state, work, markers, args.timeout, args.memory_mib, disk)


if __name__ == '__main__':
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except KeyboardInterrupt:
        raise SystemExit(130)
    except (OSError, RuntimeError, TimeoutError) as error:
        raise SystemExit(f'Trusty x86: {error}')
