"""XWD decoding tests independent of a VM and its display server."""
from pathlib import Path
import struct
import tempfile
import unittest
from framebuffer import pixels


def xwd(visual, order=0):
    colors = 256 if visual == 5 else 0
    header = struct.pack(">25I", 100, 7, 2, 24, 800, 600, 0, order, 32, 0, 32,
                         32, 3200, visual, 0xff0000, 0xff00, 0xff, 8, 256,
                         colors, 800, 600, 0, 0, 0)
    palette = b"".join(struct.pack(">IHHHBB", i * 0x010101,
        (255-i) * 257, i * 257, i * 257, 7, 0) for i in range(colors))
    data = bytearray(800 * 600 * 4)
    data[:4] = (0x123456).to_bytes(4, "little" if order == 0 else "big")
    return header + palette + data


class FramebufferTest(unittest.TestCase):
    def test_true_color_both_byte_orders(self):
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / "frame"
            for order in (0, 1):
                path.write_bytes(xwd(4, order))
                self.assertEqual(pixels(path)(0, 0), (0x12, 0x34, 0x56))

    def test_direct_color_uses_installed_component_tables(self):
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / "frame"
            path.write_bytes(xwd(5))
            self.assertEqual(pixels(path)(0, 0), (255-0x12, 0x34, 0x56))

    def test_truncated_and_unsupported_layouts_are_rejected(self):
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / "frame"
            frame = xwd(4)
            for data in (frame[:99], frame[:-1], xwd(3)):
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    pixels(path)


if __name__ == "__main__":
    unittest.main()
