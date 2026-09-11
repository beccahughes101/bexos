#!/usr/bin/env python3
import argparse
import re
import sys

PACKAGE_RE = re.compile(r"^\s*package:\s*\"([^\"]+)\"", re.MULTILINE)


def validate(manifest, declared):
    with open(manifest, "r", encoding="utf-8") as handle:
        packages = PACKAGE_RE.findall(handle.read())
    missing = [package for package in packages if package.startswith("//") and package not in declared]
    if missing:
        joined = "\n  ".join(missing)
        declared_text = "\n  ".join(sorted(declared))
        raise ValueError(
            "BootFS manifest references package labels that are not declared as Bazel inputs:\n"
            f"  {joined}\nDeclared labels:\n  {declared_text}"
        )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--declared-label", action="append", default=[])
    parser.add_argument("--declared-labels-file", action="append", default=[])
    parser.add_argument("--stamp")
    args = parser.parse_args()

    try:
        declared = set(args.declared_label)
        for labels_file in args.declared_labels_file:
            with open(labels_file, "r", encoding="utf-8") as handle:
                declared.update(line.strip() for line in handle if line.strip())
        validate(args.manifest, declared)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1

    if args.stamp:
        with open(args.stamp, "w", encoding="utf-8") as handle:
            handle.write("ok\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
