"""Read the Xvfb framebuffer, including GL scanouts absent from QMP screendump."""
import struct


def pixels(path):
    data = path.read_bytes()
    if len(data) < 100:
        raise ValueError("truncated XWD header")
    fields = struct.unpack_from(">25I", data)
    (header, version, format_, depth, width, height, xoffset, order,
     _, _, _, bits, stride, visual, red, green, blue, _, _, colors,
     *_) = fields
    if (header < 100 or version != 7 or format_ != 2 or depth != 24
            or (width, height) != (800, 600) or xoffset != 0 or order not in (0, 1)
            or bits != 32 or stride < width * 4 or visual not in (4, 5)
            or (red, green, blue) != (0xff0000, 0xff00, 0xff)):
        raise ValueError("unsupported XWD layout: " + repr(fields))
    start = header + colors * 12
    if start + stride * height > len(data):
        raise ValueError("truncated XWD pixels")
    tables = [[None] * 256 for _ in range(3)]
    if visual == 5:
        for at in range(header, start, 12):
            index, r, g, b, flags, _ = struct.unpack_from(">IHHHBB", data, at)
            for component, shift, value in [(0, 16, r), (1, 8, g), (2, 0, b)]:
                if flags & (1 << component):
                    tables[component][(index >> shift) & 255] = value >> 8
        if any(value is None for table in tables for value in table):
            raise ValueError("incomplete XWD DirectColor table")
    def pixel(x, y):
        offset = start + y * stride + x * 4
        value = int.from_bytes(data[offset:offset + 4], "little" if order == 0 else "big")
        rgb = ((value >> 16) & 255, (value >> 8) & 255, value & 255)
        return tuple(tables[i][value] for i, value in enumerate(rgb)) if visual == 5 else rgb
    return pixel


def verify(path):
    pixel = pixels(path)
    samples = [pixel(120, 100), pixel(500, 80), pixel(700, 80)]
    # Same scene and exact CPU colors as the native QEMU input fixture.
    expected = [(54, 54, 54), (60, 60, 60), (93, 93, 93)]
    return samples == expected, samples


def diagnose(path):
    pixel = pixels(path)
    ring = [(x, y) for y in range(600) for x in range(800) if pixel(x, y) == (54, 54, 54)]
    return {"ring_pixels": len(ring), "ring_bounds": (
        min(x for x, _ in ring), min(y for _, y in ring),
        max(x for x, _ in ring), max(y for _, y in ring)) if ring else None,
        "row100": [(x, pixel(x, 100)) for x in range(0, 800, 20)]}
