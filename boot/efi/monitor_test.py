"""One authenticated EFI-to-SVM boot, with explicit firmware rejection cases."""
from pathlib import Path
import signal
import sys

from test import DISABLED, REJECTED, UNPROTECTED, boot, interrupted

signal.signal(signal.SIGTERM, interrupted)
code, enrolled, signed, empty = map(Path, sys.argv[1:])
payload = signed.read_bytes()
boot(code, enrolled, payload,
     b'svm-probe: independent guest register contexts verified', 'authenticated monitor')
tampered = bytearray(payload)
offset = payload.find(b'\x7fELF\x02\x01\x01')
if offset < 0:
    raise RuntimeError('monitor ELF not embedded in signed EFI image')
# Alter an embedded monitor program header, keeping the outer PE loadable.
tampered[offset + 64] ^= 1
boot(code, enrolled, tampered, REJECTED, 'modified embedded monitor')
boot(code, empty, payload, DISABLED, 'monitor with Secure Boot disabled')
boot(code, enrolled, payload, UNPROTECTED, 'monitor with unprotected vars', flash_protected=False)
boot(code, enrolled, payload, b'svm-probe: SVM/NPT unavailable; refusing execution',
     'monitor without SVM', cpu='max,-svm')
