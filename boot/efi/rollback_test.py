"""Provision an authenticated floor, reboot, and require product rejection."""
from pathlib import Path
import shutil
import signal
import sys
import tempfile

from test import interrupted
from trusty_domain_test import authenticated_boot


def main():
    signal.signal(signal.SIGTERM, interrupted)
    code, variables, provisioner, product, rpmbd, template, disk_source, *external = map(Path, sys.argv[1:])
    # macOS limits sockaddr_un paths to 103 bytes; its default temporary root
    # alone consumes most of that budget before the per-boot socket names.
    with tempfile.TemporaryDirectory(prefix='bexos-rpmb-rollback-', dir='/tmp') as temporary:
        work = Path(temporary)
        disk = work / 'disk.img'
        shutil.copyfile(disk_source, disk)
        disk.chmod(0o600)
        avb_rejection = b'monitor-runtime: stale generation rejected by Trusty RPMB floor'
        journal_rejection = b'monitor-runtime: stale generation rejected by Trusty committed replacement floor'
        for component, control, marker, rejection in [
            ('payload', None, b'monitor-fixture: authenticated RPMB floor provisioned', avb_rejection),
            ('trusty', Path(__file__).with_name('fixture_trusty_floor.prototxt'),
             b'monitor-fixture: Trusty generation floor provisioned', avb_rejection),
            ('monitor', Path(__file__).with_name('fixture_monitor_floor.prototxt'),
             b'monitor-fixture: monitor generation floor provisioned', avb_rejection),
            ('trusty-journal', Path(__file__).with_name('fixture_trusty_journal.prototxt'),
             b'monitor-fixture: Trusty replacement journal durably committed', journal_rejection),
            ('monitor-journal', Path(__file__).with_name('fixture_monitor_journal.prototxt'),
             b'monitor-fixture: monitor replacement journal durably committed', journal_rejection),
        ]:
            case = work / component
            case.mkdir()
            state = case / 'RPMB_DATA'
            shutil.copyfile(template, state)
            state.chmod(0o600)
            first, second = case / 'provision', case / 'reject'
            first.mkdir()
            second.mkdir()
            authenticated_boot(code, variables, provisioner.read_bytes(), rpmbd, state, first,
                               boot_approval=True, external_payload=external,
                               expected_override=marker, fixture_control=control)
            authenticated_boot(code, variables, product.read_bytes(), rpmbd, state, second,
                               normal_world=True, disk=disk, boot_approval=True, secure_product=True,
                               external_payload=external,
                               expected_override=rejection)
            print(f'authenticated {component} generation rejection verified', flush=True)


if __name__ == '__main__':
    main()
