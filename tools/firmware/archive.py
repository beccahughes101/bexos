"""Canonical, bounded saved firmware archives shared by explicit Bazel refreshes.

Hashes detect damaged or mismatched saved artifacts. They do not replace the
firmware signature verification performed by the executing trust chain.
"""
import hashlib
import io
import os
from pathlib import Path
import tarfile
import tempfile

MANIFEST = 'build.prototxt'
MAX_MEMBER = 256 * 1024 * 1024
MAX_TOTAL = 512 * 1024 * 1024


def manifest(files, header):
    return (header + ''.join(
        f'artifact {{ name: "{name}" size: {len(data)} sha256: "{hashlib.sha256(data).hexdigest()}" }}\n'
        for name, data in sorted(files.items()))).encode()


def read(source, names, header):
    files = {}
    total = 0
    with tarfile.open(source, 'r:') as archive:
        for member in archive:
            if (member.name not in (*names, MANIFEST) or member.name in files
                    or not member.isfile() or member.pax_headers
                    or not 0 < member.size <= (65536 if member.name == MANIFEST else MAX_MEMBER)):
                raise ValueError(f'invalid firmware member {member.name}')
            total += member.size
            if total > MAX_TOTAL:
                raise ValueError('firmware archive exceeds size bound')
            data = archive.extractfile(member).read(member.size + 1)
            if len(data) != member.size:
                raise ValueError('truncated firmware member')
            files[member.name] = data
    if set(files) != set((*names, MANIFEST)):
        raise ValueError('incomplete firmware archive')
    recorded = files.pop(MANIFEST)
    if recorded != manifest(files, header):
        raise ValueError('firmware hash, version, architecture or variant mismatch')
    return files


def write(destination, files, header, validate):
    if (not files or any(not name or name in ('.', '..', MANIFEST)
                        or any(not (character.isascii() and (character.isalnum() or character in '._-'))
                               for character in name) for name in files)
            or any(not 0 < len(data) <= MAX_MEMBER for data in files.values())
            or sum(map(len, files.values())) > MAX_TOTAL - 65536):
        raise ValueError('invalid firmware artifact inventory')
    validate(files)
    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as output:
            temporary = Path(output.name)
            with tarfile.open(fileobj=output, mode='w', format=tarfile.USTAR_FORMAT) as archive:
                for name, data in [*sorted(files.items()), (MANIFEST, manifest(files, header))]:
                    member = tarfile.TarInfo(name)
                    member.size = len(data)
                    member.mode = 0o755 if name == 'rpmb_dev' else 0o644
                    archive.addfile(member, io.BytesIO(data))
            output.flush()
            os.fsync(output.fileno())
        validate(read(temporary, tuple(files), header))
        temporary.chmod(0o644)
        os.replace(temporary, destination)
        descriptor = os.open(destination.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def extract(destination, files, header):
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    for name, data in [*files.items(), (MANIFEST, manifest(files, header))]:
        target = destination / name
        target.write_bytes(data)
        target.chmod(0o755 if name == 'rpmb_dev' else 0o644)
