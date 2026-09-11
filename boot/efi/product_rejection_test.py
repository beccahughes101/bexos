"""Require actual product-loader and signed kernel-layout rejection evidence."""
from pathlib import Path
import signal
import sys

from payload_rejection import arguments, verify_rejections
from test import ACCEPTED, REJECTED, boot, interrupted


def main():
    signal.signal(signal.SIGTERM, interrupted)
    code, enrolled, revoked, signed, unsigned, wrong_key, *artifacts = map(Path, sys.argv[1:])
    payload = signed.read_bytes()
    for label, variables, image in [('unsigned', enrolled, unsigned),
                                    ('wrong key', enrolled, wrong_key),
                                    ('revoked', revoked, signed)]:
        boot(code, variables, image.read_bytes(), REJECTED, f'product loader {label}', memory_mib=3072)
    verify_rejections(code, enrolled, payload, artifacts[:5], 3072)
    for index, variant in enumerate(('architecture', 'handoff', 'bootfs', 'evidence', 'update')):
        kernel, metadata = artifacts[5 + 2 * index:7 + 2 * index]
        boot(code, enrolled, payload,
             b'monitor-runtime: kernel load layout rejected; refusing execution',
             f'authenticated invalid kernel {variant}',
             extra_args=arguments([kernel, artifacts[1], metadata]),
             additional_markers=[ACCEPTED], memory_mib=3072,
             forbidden_markers=[b'kernel: boot kernel_main', b'userspace: entering ring3 appd'])


if __name__ == '__main__':
    main()
