"""Enroll development certificates into an empty authenticated OVMF varstore.

This runs only at image assembly, under the trusted QEMU host. It does not
provide a guest-side bypass of authenticated UEFI variable updates.
"""
import argparse
from pathlib import Path
import struct
import uuid

AUTH_STORE = uuid.UUID('aaf32c78-947b-439a-a180-2e144ec37792')
GLOBAL = uuid.UUID('8be4df61-93ca-11d2-aa0d-00e098032b8c')
IMAGE_SECURITY = uuid.UUID('d719b2cb-3d3a-4596-a3bc-dad00e67656f')
X509 = uuid.UUID('a5c059a1-94e4-4aa7-87b5-ab155c2bf072')
OWNER = uuid.UUID('6265786f-732d-4566-892d-646576726f6f')


def signature_list(certificate):
    if not certificate or len(certificate) > 65536:
        raise ValueError('invalid certificate length')
    size = 16 + len(certificate)
    return X509.bytes_le + struct.pack('<III', 28 + size, 0, size) + OWNER.bytes_le + certificate


def variable(name, vendor, attributes, data):
    encoded = (name + '\0').encode('utf-16-le')
    timestamp = struct.pack('<H6BIh2B', 2026, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0)
    header = struct.pack('<HBBIQ', 0x55aa, 0x3f, 0, attributes, 0)
    header += timestamp + struct.pack('<III', 0, len(encoded), len(data)) + vendor.bytes_le
    # EDK2 VariableFormat.h: names/data have byte alignment on x86;
    # only the next variable header is aligned to four bytes.
    result = header + encoded + data
    return result + b'\0' * (-len(result) % 4)


def enroll(template, certificate, revoke=False):
    if len(template) < 100 or template[40:44] != b'_FVH':
        raise ValueError('invalid firmware volume')
    header_size = struct.unpack_from('<H', template, 48)[0]
    if header_size < 56 or header_size + 28 > len(template):
        raise ValueError('invalid firmware-volume header size')
    if template[header_size:header_size+16] != AUTH_STORE.bytes_le:
        raise ValueError('expected authenticated variable store')
    store_size = struct.unpack_from('<I', template, header_size+16)[0]
    start, end = header_size + 28, header_size + store_size
    if store_size < 28 or end > len(template) or start % 4:
        raise ValueError('invalid variable-store bounds')
    if template[header_size+20:header_size+22] != b'\x5a\xfe':
        raise ValueError('unformatted variable store')
    if any(byte != 0xff for byte in template[start:end]):
        raise ValueError('refusing to replace an existing variable store')
    signatures = signature_list(certificate)
    records = b''.join(variable(name, vendor, 0x27, signatures) for name, vendor in [
        ('PK', GLOBAL), ('KEK', GLOBAL), ('db', IMAGE_SECURITY),
    ])
    if revoke:
        records += variable('dbx', IMAGE_SECURITY, 0x27, signatures)
    records += variable('SecureBootEnable', uuid.UUID('f0a30bc7-af08-4556-99c4-001009c93a44'), 3, b'\x01')
    records += variable('CustomMode', uuid.UUID('c076ec0c-7028-4399-a072-71ee5c448b9f'), 3, b'\0')
    if len(records) > end-start:
        raise ValueError('certificates exceed variable-store capacity')
    result = bytearray(template)
    result[start:start+len(records)] = records
    return bytes(result)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('template', type=Path)
    parser.add_argument('certificate', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--revoke', action='store_true')
    args = parser.parse_args()
    args.output.write_bytes(enroll(args.template.read_bytes(), args.certificate.read_bytes(), args.revoke))
