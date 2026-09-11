"""OVMF authentication through actual isolated Trusty execution and RPMB I/O."""
import argparse
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

from test import ACCEPTED, DISABLED, REJECTED, UNPROTECTED, boot, interrupted
from qmp import attach_serial, detach_serial


def authenticated_boot(code, enrolled, payload, rpmbd, state, work, generation=None, normal_world=False, disk=None, boot_approval=False, secure_product=False, external_payload=None, expected_override=None, fixture_control=None, firmware=None, persistent_variables=None):
    endpoint = work / 'rpmb.sock'
    control = work / 'qmp.sock'
    child = debug_client = None
    transferred = False
    device_args = []
    payload_args = []
    fixture_args = ['-fw_cfg', f'name=opt/bexos/fixture-floor,file={fixture_control}'] if fixture_control else []
    if external_payload:
        from payload_rejection import arguments
        payload_args = arguments(external_payload[:3])
    if normal_world:
        sys.path.insert(0, str(Path(__file__).absolute().parents[2]))
        from third_party.trusty import x86_devices
        device_args = x86_devices.arguments(disk, work / "debug.sock", work / "normal-detached.sock" if secure_product else None)
    if firmware is not None:
        device_args += ['-drive', f'if=none,id=firmwaredisk,format=raw,cache=writeback,file={firmware}',
                        '-device', 'isa-ide,id=firmwarebus,iobase=0x1f0,iobase2=0x3f6,irq=14',
                        '-device', 'ide-hd,bus=firmwarebus.0,unit=0,drive=firmwaredisk']

    def start_proxy(output):
        nonlocal child, debug_client, transferred
        if secure_product and child is not None and not transferred and b'monitor-runtime: boot RPMB owner release verified' in output:
            # One helper and one connection own the persistent image. The old
            # Trusty proxy acknowledged IPC closure before detaching its UART.
            detach_serial(control, 'rpmb')
            attach_serial(control, 'normalrpmb', endpoint)
            transferred = True
        if child is not None or b'monitor-runtime: entering assigned Trusty domain' not in output:
            return
        # OVMF probes serial consoles. Attach the opaque RPMB transport only
        # after authenticated monitor entry, so terminal probes cannot corrupt
        # RPMB framing. The monitor waits for carrier before entering Trusty.
        child = subprocess.Popen([str(rpmbd.resolve()), '--dev', str(state), '--sock', str(endpoint)], stdin=subprocess.DEVNULL)
        deadline = time.monotonic() + 10
        while not endpoint.exists() or not stat.S_ISSOCK(endpoint.stat().st_mode):
            if child.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError('RPMB proxy not ready')
            time.sleep(.01)
        attach_serial(control, 'rpmb', endpoint)
        if normal_world:
            debug_client = x86_devices.connect(work / 'debug.sock')

    markers = [ACCEPTED, b'efi-loader: authenticated monitor reserved; boot services exited',
               b'monitor-runtime: entering assigned Trusty domain', b'bexos-trusty-x86: ready']
    expected = b'bexos-trusty-x86: RPMB exchange complete'
    if boot_approval and expected_override is None:
        markers.append(b'monitor-runtime: authenticated payload and Trusty rollback approval verified')
    if secure_product and expected_override is None:
        markers += [b'monitor-runtime: boot RPMB owner release verified',
                    b'kernel: secure boot evidence verified', b'kernel: RPMB anti-rollback backend verified',
                    b'kernel: x86 secure monitor hypercall transport enabled', b'teed: service ready']
    if firmware is not None and expected_override is None:
        markers += [b'bexos-trusty-x86: independent root control worker ready',
                    b'monitor-runtime: authenticated firmware selection complete; recovery instance discarded']
    if generation is not None:
        expected = b'bexos-security-acceptance: complete'
        markers += [b'bexos-authmgr-acceptance: complete',
                    f'bexos-security-acceptance: storage generation={generation}'.encode()]
    if normal_world and expected_override is None:
        markers += [b'kernel: cpu3 boot path ready', b'kernel: Q35 HPET IOAPIC interrupt verified',
                    b'kernel: cpu3 scheduler idle ready', b'userspace: entering ring3 appd', b'debugd: QEMU socket transport ready',
                    b'appd: guest persistence and disk-only application verified',
                    b'wasi-fixture: streams clocks random environment ok']
    try:
        boot(code, enrolled, payload, expected_override or expected,
             f'authenticated monitor and real Trusty generation={generation}', extra_args=(
                 '-qmp', f'unix:{control},server=on,wait=off',
                 '-chardev', f'socket,id=rpmb,path={work / "detached.sock"},server=on,wait=off',
                 '-serial', 'chardev:rpmb', *device_args, *payload_args, *fixture_args), on_output=start_proxy,
             additional_markers=markers, timeout=60 if expected_override else 600 if normal_world else 300 if generation else 60,
             memory_mib=3072 if normal_world or boot_approval else 1024,
             persistent_variables=persistent_variables,
             forbidden_markers=[b'kernel: boot kernel_main', b'userspace: entering ring3 appd'] if expected_override else [])
    finally:
        if debug_client is not None:
            debug_client.close()
        if child is not None:
            if child.poll() is None:
                child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()


def main():
    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser()
    parser.add_argument('artifacts', nargs=7, type=Path)
    parser.add_argument('--acceptance', action='store_true')
    parser.add_argument('--normal-world', action='store_true')
    parser.add_argument('--boot-approval', action='store_true')
    parser.add_argument('--secure-product', action='store_true')
    parser.add_argument('--disk', type=Path)
    parser.add_argument('--external-payload', nargs=5, type=Path)
    parser.add_argument('--resident-nucleus', action='store_true')
    args = parser.parse_args()
    acceptance, normal_world = args.acceptance, args.normal_world or args.secure_product
    memory_mib = 3072 if normal_world or args.boot_approval else 1024
    if normal_world and args.disk is None:
        parser.error('normal-world execution requires its disk fixture')
    code, enrolled, signed, empty, rpmbd, template, trusty_elf = args.artifacts
    payload = signed.read_bytes()
    with tempfile.TemporaryDirectory(prefix='bexos-efi-trusty-', dir='/tmp') as temporary:
        work = Path(temporary)
        disk = work / 'normal.img' if normal_world else None
        if disk is not None:
            shutil.copyfile(args.disk, disk)
            disk.chmod(0o600)
        state = work / 'RPMB_DATA'
        shutil.copyfile(template, state)
        state.chmod(0o600)
        firmware = variables = None
        if args.resident_nucleus:
            sys.path.insert(0, str(Path(__file__).absolute().parents[2]))
            from third_party.trusty.firmware_recovery_test import create_disk
            firmware, variables = work / 'firmware.raw', work / 'persistent-vars.fd'
            create_disk(firmware)
        for index, generation in enumerate([1, 2] if acceptance else [None, None] if args.boot_approval or args.secure_product else [None]):
            instance = work / str(index)
            instance.mkdir()
            authenticated_boot(code, enrolled, payload, rpmbd, state, instance, generation, normal_world, disk, args.boot_approval or args.secure_product, args.secure_product, args.external_payload, firmware=firmware, persistent_variables=variables)

    external_args = []
    if args.external_payload:
        from payload_rejection import arguments, verify_rejections
        verify_rejections(code, enrolled, payload, args.external_payload, memory_mib)
        external_args = arguments(args.external_payload[:3])

    for label, offset in [('monitor', payload.find(b'\x7fELF\x02\x01\x01')),
                          ('Trusty', payload.find(trusty_elf.read_bytes()))]:
        if offset < 0:
            raise RuntimeError(f'missing embedded {label} image')
        tampered = bytearray(payload)
        tampered[offset + 64] ^= 1
        boot(code, enrolled, tampered, REJECTED, f'modified embedded {label}', memory_mib=memory_mib)
    boot(code, empty, payload, DISABLED, 'Trusty with Secure Boot disabled', memory_mib=memory_mib)
    boot(code, enrolled, payload, UNPROTECTED, 'Trusty with unprotected vars', flash_protected=False, memory_mib=memory_mib)
    boot(code, enrolled, payload, b'monitor-runtime: SVM/NPT unavailable; refusing execution',
         'Trusty without isolation', cpu='max,-svm', memory_mib=memory_mib, extra_args=external_args)


if __name__ == '__main__':
    main()
