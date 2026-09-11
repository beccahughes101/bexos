"""Collect pinned upstream license texts and source notices through Bazel."""
import pathlib
import re
import sys


def main():
    root = pathlib.Path(sys.argv[1]).parent
    output = pathlib.Path(sys.argv[2])
    sections = ["Mesa Venus: pinned upstream license texts and source notices.\n"
                "This collection includes notices from supporting source files; "
                "it does not claim every listed file is linked into a particular image.\n"]
    for path in sorted((root / "licenses").rglob("*")):
        if path.is_file():
            sections.append(f"\n--- {path.relative_to(root)} ---\n" + path.read_text())
    for directory in ("src/virtio", "src/vulkan", "src/util", "src/compiler", "src/c11", "include"):
        for path in sorted((root / directory).rglob("*")):
            if path.suffix not in (".c", ".h", ".cpp", ".hpp", ".rs") or not path.is_file():
                continue
            text = path.read_text(errors="replace")
            notices = [m.group(0) for m in re.finditer(r"/\*.*?\*/|(?://[^\n]*\n)+", text, re.S)
                       if "copyright" in m.group(0).lower() or "SPDX-License-Identifier" in m.group(0)]
            if notices:
                sections.append(f"\n--- {path.relative_to(root)} ---\n" + "\n".join(notices))
    output.write_text("\n".join(sections))


if __name__ == "__main__":
    main()
