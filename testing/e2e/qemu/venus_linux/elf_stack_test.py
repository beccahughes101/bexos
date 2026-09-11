import struct
import tempfile
import unittest
from pathlib import Path
from elf_stack import reserve_thread_stack


class StackReservation(unittest.TestCase):
    def test_only_stack_size_changes_and_invalid_inputs_are_untouched(self):
        data = bytearray(256)
        data[:6] = b'\x7fELF\x02\x01'
        struct.pack_into('<H', data, 18, 183)
        struct.pack_into('<Q', data, 32, 64)
        struct.pack_into('<HH', data, 54, 56, 2)
        struct.pack_into('<IIQQQQQQ', data, 64, 1, 5, 0, 0, 0, 256, 256, 4096)
        struct.pack_into('<IIQQQQQQ', data, 120, 0x6474e551, 6, 0, 0, 0, 0, 0, 16)
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / 'renderer'
            path.write_bytes(data)
            reserve_thread_stack(path, 8 << 20)
            expected = data.copy()
            struct.pack_into('<Q', expected, 160, 8 << 20)
            self.assertEqual(path.read_bytes(), expected)
            for field, value, encoding in [(124, 7, '<I'), (54, 0, '<H'), (18, 62, '<H'), (32, 240, '<Q')]:
                invalid = data.copy()
                struct.pack_into(encoding, invalid, field, value)
                path.write_bytes(invalid)
                with self.assertRaises(ValueError):
                    reserve_thread_stack(path, 8 << 20)
                self.assertEqual(path.read_bytes(), invalid)


if __name__ == '__main__':
    unittest.main()
