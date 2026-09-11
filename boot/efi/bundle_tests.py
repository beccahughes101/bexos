"""Structural cache checks complement guest-side signature acceptance tests."""
import struct
import unittest

import bundle


def elf(payload=b''):
    image = bytearray(64)
    image[:7] = b'\x7fELF\x02\x01\x01'
    struct.pack_into('<HH', image, 16, 2, 62)
    return bytes(image) + payload


def loader(payload):
    loader = bytearray(512)
    loader[:2] = b'MZ'
    struct.pack_into('<I', loader, 60, 64)
    loader[64:68] = b'PE\0\0'
    struct.pack_into('<H', loader, 68, 0x8664)
    struct.pack_into('<H', loader, 88, 0x20b)
    loader.extend(payload)
    loader.extend(b'\0' * (-len(loader) % 8))
    struct.pack_into('<II', loader, 88 + 144, len(loader), 8)
    loader.extend(struct.pack('<IHH', 8, 0x200, 2))
    return bytes(loader)


def fixture():
    root = b'R' * 520
    trusty = elf(b'Trusty')
    monitor = elf(trusty + root)
    files = {name: b'fixture-' + name.encode() for name in bundle.NAMES}
    files.update({'loader.efi': loader(monitor), 'monitor.elf': monitor,
                  'rollback_loader.efi': loader(elf(trusty + root + b'rollback fixture')),
                  'trusty.elf': trusty, 'boot_root.avbpubkey': root})
    candidates = [('trusty', 'trusty.candidate.elf', 'trusty.replacement.fw'),
                  ('hypervisor', 'monitor.policy.elf', 'hypervisor.replacement.fw'),
                  ('hypervisor', 'monitor.fault.elf', 'hypervisor.fault.fw'),
                  ('hypervisor', 'monitor.hang.elf', 'hypervisor.hang.fw'),
                  ('hypervisor', 'monitor.successor.elf', 'hypervisor.successor.fw'),
                  ('hypervisor', 'monitor.fault.elf', 'hypervisor.successor_fault.fw')]
    for component, name, envelope in candidates:
        image = elf(name.encode())
        files[name] = image
        metadata = b'\0' * 256 + root + (component + '/x86_64/state/1').encode()
        files[envelope] = b'BEXFW001' + struct.pack('<QQQ', len(metadata), len(image), 0) + metadata + image
    return files


class BundleTests(unittest.TestCase):
    def test_complete_matching_closure_and_manifest_contract(self):
        bundle.validate(fixture())
        self.assertIn('monitor_abi_version: 1\n', bundle.header('standard'))
        self.assertIn('monitor_architecture_id: 2\n', bundle.header('acceptance'))
        with self.assertRaises(ValueError):
            bundle.header('aarch64')

    def test_wrong_architecture_unsigned_and_mismatched_embedded_images(self):
        for name, offset in [('monitor.elf', 18), ('trusty.elf', 18),
                             ('loader.efi', 68), ('loader.efi', 88),
                             ('loader.efi', 88 + 144), ('boot_root.avbpubkey', 0)]:
            files = fixture()
            image = bytearray(files[name])
            image[offset] ^= 0xff
            files[name] = bytes(image)
            with self.assertRaises(ValueError, msg=f'{name} at {offset}'):
                bundle.validate(files)
        for name in ('loader.efi', 'monitor.elf', 'trusty.elf', 'boot_root.avbpubkey'):
            files = fixture()
            files[name] = files[name][:12]
            with self.assertRaises(ValueError):
                bundle.validate(files)
        files = fixture()
        del files['RPMB_DATA']
        with self.assertRaises(ValueError):
            bundle.validate(files)
        files = fixture()
        files['rollback_loader.efi'] = files['loader.efi']
        with self.assertRaises(ValueError):
            bundle.validate(files)
        for name in ('trusty.replacement.fw', 'hypervisor.replacement.fw'):
            files = fixture()
            files[name] = files[name][:-1] + b'X'
            with self.assertRaises(ValueError):
                bundle.validate(files)


if __name__ == '__main__':
    unittest.main()
