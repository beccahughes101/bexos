"""Create a deterministic newc initramfs from Bazel-verified APKs.

Package scripts are never executed. Files are spooled by identity, so archive
symlinks cannot redirect extraction outside the action directory.
"""
import argparse
import gzip
from pathlib import Path, PurePosixPath
import shutil
import stat
import tarfile
import tempfile
from elf_stack import reserve_thread_stack


def build(args):
    with tempfile.TemporaryDirectory() as work:
        entries = {}
        links = {}
        next_spool = 0
        for package in args.packages:
            # APK v2 concatenates signature, control and payload gzip members.
            with gzip.open(package, "rb") as raw, tarfile.open(fileobj=raw, mode="r|", ignore_zeros=True) as archive:
                for member in archive:
                    name = str(PurePosixPath(member.name))
                    if name.startswith("."):
                        continue
                    if name.startswith("/") or ".." in PurePosixPath(name).parts:
                        raise ValueError("unsafe package path: " + name)
                    if member.isdir():
                        entries[name] = (stat.S_IFDIR | member.mode, b"")
                    elif member.issym():
                        entries[name] = (stat.S_IFLNK | member.mode, member.linkname.encode())
                    elif member.islnk():
                        links[name] = member.linkname
                    elif member.isfile():
                        spool = Path(work) / str(next_spool)
                        next_spool += 1
                        with archive.extractfile(member) as source, spool.open("wb") as target:
                            shutil.copyfileobj(source, target)
                        entries[name] = (stat.S_IFREG | member.mode, spool)
                    else:
                        raise ValueError("unexpected package node: " + name)
        for name, target in links.items():
            if target not in entries or stat.S_IFMT(entries[target][0]) != stat.S_IFREG:
                raise ValueError("invalid package hard link: " + name)
            entries[name] = entries[target]
        reserve_thread_stack(entries['usr/libexec/virgl_render_server'][1], 8 * 1024 * 1024)
        shutil.copyfile(entries.pop("boot/vmlinuz-virt")[1], args.kernel)
        for name, source, mode in [("init", args.init, 0o755), ("fixture/check.py", args.check, 0o644),
                                   ("fixture/vulkan_probe.py", args.probe, 0o644),
                                   ("fixture/packages.prototxt", args.lock, 0o644)]:
            entries[name] = (stat.S_IFREG | mode, Path(source))
        for payload in args.payload:
            name, source = payload.split("=", 1)
            if "/" in name or name in (".", ".."):
                raise ValueError("invalid fixture payload name")
            entries["fixture/" + name] = (stat.S_IFREG | (0o755 if name in args.executable else 0o644), Path(source))
        if any("fixture/" + name not in entries for name in args.executable):
            raise ValueError("executable fixture payload is missing")
        for name in list(entries):
            for parent in PurePosixPath(name).parents:
                if str(parent) != ".":
                    entries.setdefault(str(parent), (stat.S_IFDIR | 0o755, b""))
        # /dev is populated by devtmpfs before Python or QEMU runs.
        for name in ("dev", "proc", "sys", "run", "tmp", "var", "var/tmp"):
            entries[name] = (stat.S_IFDIR | (0o1777 if name in ("tmp", "var/tmp") else 0o755), b"")
        with open(args.initramfs, "wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0, compresslevel=1) as out:
            def emit(name, mode, source, inode):
                size = source.stat().st_size if isinstance(source, Path) else len(source)
                encoded = name.encode() + b"\0"
                fields = [inode, mode, 0, 0, 1, 0, size, 0, 0, 0, 0, len(encoded), 0]
                header = b"070701" + b"".join(f"{value:08x}".encode() for value in fields)
                out.write(header + encoded)
                out.write(b"\0" * (-(len(header) + len(encoded)) % 4))
                if isinstance(source, Path):
                    with source.open("rb") as content:
                        shutil.copyfileobj(content, out)
                else:
                    out.write(source)
                out.write(b"\0" * (-size % 4))
            for inode, (name, (mode, source)) in enumerate(sorted(entries.items()), 1):
                emit(name, mode, source, inode)
            emit("TRAILER!!!", 0, b"", len(entries) + 1)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    for name in ("kernel", "initramfs", "init", "check", "probe", "lock"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("packages", nargs="+")
    parser.add_argument("--payload", action="append", default=[])
    parser.add_argument("--executable", action="append", default=[])
    build(parser.parse_args())
