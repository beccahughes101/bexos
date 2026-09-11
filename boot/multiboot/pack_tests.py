import struct
import unittest
from pack import inspect


def elf(machine=62, address=0x200000):
    data = bytearray(256)
    data[:7] = b'\x7fELF\x02\x01\x01'
    struct.pack_into('<HHIQQQ', data, 16, 2, machine, 1, address, 64, 0)
    struct.pack_into('<HHHHHH', data, 52, 64, 56, 1, 0, 0, 0)
    struct.pack_into('<IIQQQQQQ', data, 64, 1, 5, 128, address, address, 128, 256, 4096)
    return data


class PackerTests(unittest.TestCase):
    def test_physical_entry_and_bss(self):
        entry, segments = inspect(elf(), True)
        self.assertEqual(entry, 0x200000)
        self.assertEqual(segments, [(0x200000, 128, 128, 256)])

    def test_trusty_physical_entry_with_high_virtual_segments(self):
        data = elf()
        struct.pack_into('<Q', data, 64+16, 0xffffffff80000000)
        self.assertEqual(inspect(data, True)[0], 0x200000)
        struct.pack_into('<Q', data, 24, 0x200080)
        with self.assertRaises(ValueError):
            inspect(data, True)

    def test_wrong_architecture_and_loader_overlap(self):
        for data in [elf(machine=183), elf(address=0x30000000), elf()[:70]]:
            with self.assertRaises(ValueError):
                inspect(data, True)

    def test_multiboot2_header_required_for_bexos(self):
        data = elf()
        with self.assertRaisesRegex(ValueError, 'Multiboot2'):
            inspect(data, False)
        struct.pack_into('<IIIIHHI', data, 128, 0xe85250d6, 0, 24, -(0xe85250d6+24)&0xffffffff, 0, 0, 8)
        self.assertEqual(inspect(data, False)[0], 0x200000)
        data[140] ^= 1
        with self.assertRaises(ValueError):
            inspect(data, False)

    def test_header_requires_terminating_tag(self):
        data = elf()
        struct.pack_into('<IIIIHHI', data, 128, 0xe85250d6, 0, 24, -(0xe85250d6+24)&0xffffffff, 9, 1, 8)
        with self.assertRaisesRegex(ValueError, 'Multiboot2'):
            inspect(data, False)

    def test_unsupported_required_header_tag_is_rejected(self):
        data = elf()
        struct.pack_into('<IIIIHHI', data, 128, 0xe85250d6, 0, 32, -(0xe85250d6+32)&0xffffffff, 9, 0, 8)
        struct.pack_into('<HHI', data, 152, 0, 0, 8)
        with self.assertRaisesRegex(ValueError, 'unsupported required'):
            inspect(data, False)

    def test_entry_must_be_executable_and_file_backed(self):
        data = elf()
        struct.pack_into('<I', data, 68, 4)
        with self.assertRaises(ValueError):
            inspect(data, True)
        data = elf()
        struct.pack_into('<Q', data, 24, 0x200080)
        with self.assertRaises(ValueError):
            inspect(data, True)


if __name__ == '__main__':
    unittest.main()
