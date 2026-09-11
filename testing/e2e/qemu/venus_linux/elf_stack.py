"""Set the musl thread-stack reservation in a derived fixture ELF.

LLVM 22's AArch64 assembly finalizer alone uses about 500 KiB of stack. musl
uses PT_GNU_STACK.p_memsz for its default pthread stack reservation. Modify
only this metadata in the Bazel-generated image; pinned package inputs and
executable code remain intact. The reservation is demand paged by Linux.
"""
import struct


def reserve_thread_stack(path, size):
    if not 1 << 20 <= size <= 8 << 20 or size % 4096:
        raise ValueError('invalid renderer thread stack reservation')
    with path.open('r+b') as output:
        header = output.read(64)
        if len(header) != 64 or header[:6] != b'\x7fELF\x02\x01':
            raise ValueError('renderer must be a little-endian ELF64 file')
        if struct.unpack_from('<H', header, 18)[0] != 183:
            raise ValueError('renderer must be AArch64')
        offset = struct.unpack_from('<Q', header, 32)[0]
        entry_size, count = struct.unpack_from('<HH', header, 54)
        if entry_size != 56 or count > 128 or offset + count * entry_size > path.stat().st_size:
            raise ValueError('invalid renderer program headers')
        output.seek(offset)
        headers = list(struct.iter_unpack('<IIQQQQQQ', output.read(count * entry_size)))
        stacks = [(i, h) for i, h in enumerate(headers) if h[0] == 0x6474e551]
        if len(stacks) != 1 or stacks[0][1][1] != 6:
            raise ValueError('renderer needs one nonexecutable read/write GNU stack')
        index, stack = stacks[0]
        if stack[6] > size:
            raise ValueError('refusing to shrink the renderer thread stack')
        output.seek(offset + index * entry_size + 40)
        output.write(struct.pack('<Q', size))
