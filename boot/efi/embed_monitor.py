"""Bazel-only generation of a validated, embedded x86 monitor payload."""
import importlib.util
from pathlib import Path
import struct
import sys


def embed(source, directory, validator, *bank_args):
    spec = importlib.util.spec_from_file_location('elf_validation', validator)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    data = Path(source).read_bytes()
    if len(data) > 128 * 1024 * 1024:
        raise ValueError('monitor ELF exceeds bounded loader payload')
    entry, segments = module.inspect(data, True)
    # This monitor is identity linked, unlike high-half Trusty.
    phoff = struct.unpack_from('<Q', data, 32)[0]
    count = struct.unpack_from('<H', data, 56)[0]
    for i in range(count):
        kind, _, _, virtual, physical = struct.unpack_from('<IIQQQ', data, phoff + i * 56)
        if kind == 1 and virtual != physical:
            raise ValueError('monitor must have identity-linked load segments')
    base = min(segment[0] for segment in segments) & ~4095
    end = (max(segment[0] + segment[3] for segment in segments) + 4095) & ~4095
    if len(bank_args) % 2:
        raise ValueError('domain reservations require base and length pairs')
    banks = []
    for index in range(0, len(bank_args), 2):
        address, length = (int(value, 0) for value in bank_args[index:index + 2])
        if address < 0x200000 or length <= 0 or (address | length) & 4095 or address + length > 0xc0000000:
            raise ValueError('invalid domain RAM reservation')
        if address < end and base < address + length:
            raise ValueError('domain RAM overlaps monitor')
        if any(address < a + n and a < address + length for a, n in banks):
            raise ValueError('overlapping domain RAM reservations')
        banks.append((address, length))
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'monitor.bin').write_bytes(data)
    (directory / 'monitor_payload.h').write_text(
        f'#define MONITOR_ENTRY {entry}u\n#define MONITOR_BASE {base}u\n'
        f'#define MONITOR_PAGES {(end-base)//4096}u\n#define MONITOR_SEGMENTS {len(segments)}u\n'
        'struct monitor_segment { unsigned address, offset, filesz, memsz; };\n'
        'static const struct monitor_segment monitor_segments[] = {\n'
        + ''.join('{%du,%du,%du,%du},\n' % segment for segment in segments) + '};\n'
        + f'#define MONITOR_BANKS {len(banks)}u\n'
        + 'struct domain_bank { unsigned address, pages; };\n'
        + 'static const struct domain_bank domain_banks[] = {\n'
        + ''.join('{%du,%du},\n' % (address, length // 4096) for address, length in banks)
        + '{0,0}};\n')
    (directory / 'monitor_payload.S').write_text(
        '.section .rdata,"dr"\n.global monitor_payload\nmonitor_payload:\n.incbin "'
        + str((directory / 'monitor.bin').resolve()) + '"\n')


if __name__ == '__main__':
    embed(*sys.argv[1:])
