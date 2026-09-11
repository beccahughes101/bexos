"""Check retained hardware references in the actual Bazel-linked ELF images."""
from pathlib import Path
import struct
import sys


def verify(path, secure, nucleus=False):
    data = Path(path).read_bytes()
    assert data[:7] == b'\x7fELF\x02\x01\x01'
    assert struct.unpack_from('<HH', data, 16) == (2, 62)
    phoff, shoff = struct.unpack_from('<QQ', data, 32)
    phsize, phnum, shsize, shnum, names_index = struct.unpack_from('<HHHHH', data, 54)
    assert phsize == 56 and shsize == 64 and 0 < names_index < shnum
    headers = [struct.unpack_from('<IIQQQQIIQQ', data, shoff + i * shsize)
               for i in range(shnum)]
    names = headers[names_index]
    strings = data[names[4]:names[4] + names[5]]
    sections = {strings[h[0]:].split(b'\0', 1)[0].decode(): h for h in headers}
    loads = [struct.unpack_from('<IIQQQQQQ', data, phoff + i * phsize)
             for i in range(phnum)]
    loads = [h for h in loads if h[0] == 1 and h[6]]
    for _, flags, offset, virtual, physical, filesz, memsz, _ in loads:
        assert virtual == physical and filesz <= memsz and offset + filesz <= len(data)
        assert flags & 3 != 3, 'root load segment is writable and executable'
        assert ((0x18000000 <= physical < physical + memsz <= 0x20000000 or
                 flags & 1 == 0 and 0x04200000 <= physical < physical + memsz <= 0x14800000) if nucleus else
                (0x04000000 <= physical < physical + memsz <= 0x18000000
                 or 0x18000000 <= physical < physical + memsz <= 0x18200000)), \
            'root load segment escapes image/resident assignments'

    required = {
        '.resident.text': (0x18000000, 0x1000),
        '.resident.gdt': (0x18001000, 0x1000),
        '.resident.idt': (0x18002000, 0x1000),
        '.resident.clock': (0x18003000, 0x1000),
        '.resident.root': (0x18004000, 0xc000),
        '.resident.iopm': (0x18010000, 0x3000),
        '.resident.msrpm': (0x18013000, 0x3000),
        '.resident.tss': (0x18016000, 0x1000),
        '.resident.hsave': (0x18018000, 0x1000),
        '.resident.host': (0x18019000, 0x1000),
        '.resident.trusty_cpu': (0x1801a000, 0x1000),
        '.resident.normal_cpus': (0x1801b000, 0x4000),
        '.resident.trusty_npt': (0x18020000, 0x20000),
        '.resident.normal_npt': (0x18040000, 0x20000),
        '.resident.dma': (0x18060000, 0x10000),
        '.resident.pci_policy': (0x18070000, 0x1000),
        '.resident.exceptions': (0x1807a000, 0x6000),
    }
    if secure:
        required['.resident.approvals'] = (0x18074000, 0x1000)
    if nucleus and secure:
        required['.resident.trial_uart'] = (0x18071000, 0x3000)
    for name, (address, capacity) in required.items():
        h = sections[name]
        flags, actual, size = h[2], h[3], h[5]
        assert actual == address and 0 < size <= capacity, name
        assert flags & 2 and bool(flags & 4) == (name == '.resident.text'), name
        assert any(p[4] <= actual and actual + size <= p[4] + p[6] for p in loads), name

    # Root faults/NMIs must not fetch instructions from the retiring image.
    symbols = {}
    for h in headers:
        if h[1] != 2:
            continue
        table = headers[h[6]]
        names = data[table[4]:table[4] + table[5]]
        assert h[9] == 24
        for at in range(h[4], h[4] + h[5], h[9]):
            name, _, _, _, value, _ = struct.unpack_from('<IBBHQQ', data, at)
            symbols[names[name:].split(b'\0', 1)[0]] = value
    for name in (b'bexos_monitor_nmi', b'bexos_monitor_root_fault'):
        assert 0x18000000 <= symbols[name] < 0x18001000, name
    assert 0x18003000 <= symbols[b'BEXOS_MONITOR_NMI_TICKS'] < 0x18004000
    if nucleus:
        for name in (b'policy_guard_call', b'policy_guard_abort'):
            assert 0x18000000 <= symbols[name] < 0x18001000, name
        for name in (b'runtime_main', b'runtime_stack_end'):
            assert 0x18200000 <= symbols[name] < 0x20000000, name
        assert struct.unpack_from('<Q', data, 24)[0] == 0x18200000
    print(f'resident-layout: protected hardware references verified in {Path(path).name}')


if sys.argv[1] == '--nucleus':
    assert len(sys.argv) == 4, 'expected development and product nucleus ELF images'
    verify(sys.argv[2], False, True)
    verify(sys.argv[3], True, True)
else:
    assert len(sys.argv) == 3, 'expected product and development ELF images'
    verify(sys.argv[1], True)
    verify(sys.argv[2], False)
