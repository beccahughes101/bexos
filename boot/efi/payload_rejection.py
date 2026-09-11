"""Guest-side authentication failures for independently supplied boot bytes."""
from pathlib import Path
import tempfile

from test import ACCEPTED, boot


def arguments(paths):
    return [argument for name, path in zip(('kernel', 'bootfs', 'vbmeta'), paths)
            for argument in ('-fw_cfg', f'name=opt/bexos/{name},file={path.resolve()}')]


def verify_rejections(code, enrolled, image, artifacts, memory_mib):
    sources = artifacts[:3]
    authenticated = b'monitor-runtime: boot payload authentication rejected'
    snapshot = b'monitor-runtime: boot payload snapshot rejected'
    # Expected failure markers come from the executing authenticated monitor.
    # Missing files, invalid signatures and invalid signed policy are distinct
    # from setup failures, QEMU exit or a timeout, which all fail the test.
    with tempfile.TemporaryDirectory(prefix='bexos-boot-rejection-') as temporary:
        work = Path(temporary)
        for index, name in enumerate(('kernel', 'BootFS', 'vbmeta')):
            original = sources[index].read_bytes()
            modified = bytearray(original)
            modified[len(modified) // 2] ^= 1
            for label, data, expected in (
                ('modified', modified, authenticated),
                ('truncated', original[:-1], authenticated),
                ('appended', original + b'\0', authenticated),
            ):
                path = work / f'{name}-{label}'
                path.write_bytes(data)
                paths = list(sources)
                paths[index] = path
                boot(code, enrolled, image, expected, f'{label} external {name}',
                     extra_args=arguments(paths), additional_markers=[ACCEPTED],
                     memory_mib=memory_mib)
        for label, metadata in zip(('signed wrong platform policy', 'signed zero generation'), artifacts[3:]):
            boot(code, enrolled, image, authenticated, label,
                 extra_args=arguments([*sources[:2], metadata]),
                 additional_markers=[ACCEPTED], memory_mib=memory_mib)
        boot(code, enrolled, image, snapshot, 'missing external payloads',
             additional_markers=[ACCEPTED], memory_mib=memory_mib)
