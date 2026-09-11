import struct
from pathlib import Path
import sys
import tempfile
import unittest
from embed_monitor import embed

VALIDATOR = Path(sys.argv.pop(1)).resolve()


def elf():
    data = bytearray(256)
    data[:7] = b'\x7fELF\x02\x01\x01'
    struct.pack_into('<HHIQQQ', data, 16, 2, 62, 1, 0x4000000, 64, 0)
    struct.pack_into('<HHHHHH', data, 52, 64, 56, 1, 0, 0, 0)
    struct.pack_into('<IIQQQQQQ', data, 64, 1, 5, 128, 0x4000000, 0x4000000, 128, 8192, 4096)
    return data


class EmbedTests(unittest.TestCase):
    def run_embed(self, data, *banks):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'source.elf'
            source.write_bytes(data)
            embed(source, root / 'output', VALIDATOR, *map(str, banks))
            self.assertEqual((root / 'output/monitor.bin').read_bytes(), data)
            return (root / 'output/monitor_payload.h').read_text()

    def test_reserves_bss_and_binds_exact_file(self):
        header = self.run_embed(elf())
        self.assertIn('#define MONITOR_PAGES 2u', header)
        self.assertIn('#define MONITOR_ENTRY 67108864u', header)

    def test_rejects_a_virtual_entry_without_required_page_tables(self):
        data = elf()
        struct.pack_into('<Q', data, 80, 0xffffffff80000000)
        with self.assertRaisesRegex(ValueError, 'identity-linked'):
            self.run_embed(data)

    def test_rejects_wrong_architecture_truncation_and_nonexecutable_entry(self):
        wrong_arch = elf()
        struct.pack_into('<H', wrong_arch, 18, 183)
        no_execute = elf()
        struct.pack_into('<I', no_execute, 68, 4)
        for data in [wrong_arch, no_execute, elf()[:120]]:
            with self.assertRaises(ValueError):
                self.run_embed(data)

    def test_reserves_each_domain_bank(self):
        header = self.run_embed(elf(), 0x20000000, 0x10000000, 0x40000000, 0x40000000)
        self.assertIn('#define MONITOR_BANKS 2u', header)
        self.assertIn('{536870912u,65536u}', header)
        self.assertIn('{1073741824u,262144u}', header)

    def test_rejects_invalid_or_overlapping_reservations(self):
        for banks in [
            (0x20000000,), (0x1000, 0x1000), (0x20000000, 0),
            (0x20000001, 0x1000), (0x20000000, 0x1001),
            (0xb0000000, 0x20000000), (0x4000000, 0x1000),
            (0x3fff000, 0x2000),
            (0x20000000, 0x2000, 0x20001000, 0x1000),
        ]:
            with self.subTest(banks=banks), self.assertRaises(ValueError):
                self.run_embed(elf(), *banks)


if __name__ == '__main__':
    unittest.main()
