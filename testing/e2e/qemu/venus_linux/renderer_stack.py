"""Read bounded AArch64 stack traces from this disposable fixture's renderer.

Only a selected busy renderer thread is paused, then detached in finally. No
stack contents or environment values are logged: frames become module/symbol
offsets. This is diagnostic evidence, never a rendering acceptance condition.
"""
import ctypes
import functools
import os
from pathlib import Path
import struct
import time


@functools.lru_cache(maxsize=4)
def symbols(path):
    with open(path, 'rb') as source:
        header = source.read(64)
        if header[:6] != b'\x7fELF\x02\x01':
            return []
        offset = struct.unpack_from('<Q', header, 40)[0]
        size, count = struct.unpack_from('<HH', header, 58)
        if size != 64 or count > 4096:
            return []
        source.seek(offset)
        sections = list(struct.iter_unpack('<IIQQQQIIQQ', source.read(size * count)))
        result = []
        for section in sections:
            if section[1] != 11 or section[9] != 24 or section[5] > 16 * 1024 * 1024:
                continue
            names = sections[section[6]]
            if names[5] > 32 * 1024 * 1024:
                continue
            source.seek(names[4])
            strings = source.read(names[5])
            source.seek(section[4])
            for name, info, _, index, value, length in struct.iter_unpack('<IBBHQQ', source.read(section[5])):
                if info & 15 != 2 or not index or not length:
                    continue
                end = strings.find(b'\0', name)
                result.append((value, length, strings[name:end].decode(errors='replace')))
        return result


def capture(pid, tid, stopped=False):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.ptrace.restype = ctypes.c_long
    libc.ptrace.argtypes = [ctypes.c_uint, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p]
    attached = False
    try:
        if not stopped:
            if libc.ptrace(16, tid, None, None) != 0:
                raise OSError(ctypes.get_errno(), 'attach renderer thread')
            attached = True
            deadline = time.monotonic() + 1
            while not os.waitpid(tid, 0x40000000 | os.WNOHANG)[0]:
                if time.monotonic() >= deadline:
                    raise TimeoutError('renderer thread stop')
                time.sleep(.001)
        class Iovec(ctypes.Structure):
            _fields_ = [('base', ctypes.c_void_p), ('size', ctypes.c_size_t)]
        registers = (ctypes.c_uint64 * 34)()
        vector = Iovec(ctypes.addressof(registers), ctypes.sizeof(registers))
        if libc.ptrace(0x4204, tid, 1, ctypes.byref(vector)) != 0:
            raise OSError(ctypes.get_errno(), 'renderer registers')
        mappings = []
        for line in (Path('/proc') / str(pid) / 'maps').read_text().splitlines():
            fields = line.split(maxsplit=5)
            if len(fields) < 6 or not fields[5].startswith('/') or 'x' not in fields[1]:
                continue
            begin, end = (int(v, 16) for v in fields[0].split('-'))
            mappings.append((begin, end, int(fields[2], 16), fields[5]))
        def describe(address):
            for begin, end, offset, path in mappings:
                if begin <= address < end:
                    relative = address - begin + offset
                    for value, length, name in symbols(path):
                        if value <= relative < value + length:
                            return f'{path}:{name}+{relative - value:#x}'
                    return f'{path}+{relative:#x}'
            return f'{address:#x}'
        frames = [describe(registers[32])]
        frame, stack = registers[29], registers[31]
        for _ in range(31):
            if frame % 16 or not stack <= frame < stack + 16 * 1024 * 1024:
                break
            ctypes.set_errno(0)
            previous = libc.ptrace(2, tid, frame, None) & ((1 << 64) - 1)
            address = libc.ptrace(2, tid, frame + 8, None) & ((1 << 64) - 1)
            if ctypes.get_errno():
                break
            frames.append(describe(address))
            if previous <= frame:
                break
            frame = previous
        print('VENUS_RENDERER_STACK', {'pid': pid, 'tid': tid, 'frames': frames}, flush=True)
    except (OSError, ValueError, TimeoutError) as error:
        print('VENUS_RENDERER_STACK_UNAVAILABLE', str(error), flush=True)
    finally:
        if attached:
            libc.ptrace(17, tid, None, None)
