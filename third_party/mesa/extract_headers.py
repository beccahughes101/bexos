"""Extract only regular GNU ABI headers and their license from a pinned deb."""
from pathlib import Path, PurePosixPath
import io
import sys
import tarfile


def extract(archive, destination):
    data = archive.read_bytes()
    if data[:8] != b"!<arch>\n":
        raise ValueError("invalid Debian archive")
    offset = 8
    payload = None
    while offset < len(data):
        header = data[offset:offset + 60]
        if len(header) != 60 or header[58:] != b"`\n":
            raise ValueError("invalid ar member")
        size = int(header[48:58])
        if size < 0 or offset + 60 + size > len(data):
            raise ValueError("truncated ar member")
        name = header[:16].decode("ascii").strip().rstrip("/")
        if name.startswith("data.tar."):
            if payload is not None:
                raise ValueError("duplicate Debian payload")
            payload = data[offset + 60:offset + 60 + size]
        offset += 60 + size + size % 2
    if payload is None:
        raise ValueError("missing Debian payload")
    count = 0
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as members:
        for member in members:
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts:
                raise ValueError("unsafe Debian member")
            name = str(path)
            if not (name.startswith("usr/include/") or name in ("usr/share/doc/libc6-dev/copyright", "usr/share/doc/linux-libc-dev/copyright")):
                continue
            if member.isdir():
                continue
            if not (member.isfile() or member.issym() or member.islnk()):
                raise ValueError("nonregular header: " + name)
            output = destination / name
            output.parent.mkdir(parents=True, exist_ok=True)
            # tarfile resolves links within its member index, never through
            # the host filesystem. Materialize bytes instead of archive links.
            output.write_bytes(members.extractfile(member).read())
            count += 1
    if count < 400:
        raise ValueError("incomplete GNU header package")


if __name__ == "__main__":
    extract(Path(sys.argv[1]), Path.cwd())
