"""Select Bazel build and non-E2E test targets affected by a pull request."""

import argparse
from pathlib import Path
import subprocess
import sys


GLOBAL_FILES = {
    ".bazelrc",
    ".bazelversion",
    "MODULE.bazel",
    "MODULE.bazel.lock",
    "WORKSPACE",
    "WORKSPACE.bazel",
}
IGNORED_PREFIXES = ("docs/",)


def run(command, cwd):
    return subprocess.run(
        command,
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout


def changed_paths(base, head, root, runner=run):
    output = runner(
        ["git", "diff", "--name-status", "--find-renames", "-z", f"{base}...{head}"],
        root,
    )
    fields = output.split("\0")
    if fields and not fields[-1]:
        fields.pop()
    changes = []
    index = 0
    while index < len(fields):
        status = fields[index]
        index += 1
        if status.startswith(("R", "C")):
            old_path, new_path = fields[index:index + 2]
            index += 2
            changes.append((status[0], old_path))
            changes.append((status[0], new_path))
        else:
            changes.append((status[0], fields[index]))
            index += 1
    return changes


def package_pattern(path, root):
    candidate = (root / path).parent
    while candidate != root and not any(
        (candidate / name).is_file() for name in ("BUILD.bazel", "BUILD")
    ):
        candidate = candidate.parent
    relative = candidate.relative_to(root).as_posix()
    return f"//{relative}:*" if relative != "." else "//:*"


def is_submodule(path, root, runner=run):
    output = runner(["git", "ls-files", "--stage", "--", path], root)
    return any(line.startswith("160000 ") for line in output.splitlines())


def requires_full_build(changes, root, runner=run):
    for status, path in changes:
        name = Path(path).name
        if status in ("D", "R") or name in GLOBAL_FILES or path.endswith(".bzl"):
            return True
        if is_submodule(path, root, runner):
            return True
    return False


def query(expression, root, bazel, runner=run):
    output = runner([bazel, "query", expression, "--output=label"], root)
    return sorted({line for line in output.splitlines() if line.startswith("//")})


def compute_targets(changes, root, bazel="bazel", runner=run):
    if not changes:
        return [], []
    if requires_full_build(changes, root, runner):
        return ["//..."], ["//..."]

    mapped_changes = [
        (status, path)
        for status, path in changes
        if not path.startswith(IGNORED_PREFIXES)
    ]
    if not mapped_changes:
        return [], []

    seeds = {package_pattern(path, root) for _, path in mapped_changes}
    if any(path.startswith((".github/", "tools/ci/")) for _, path in mapped_changes):
        seeds.add("//tools/ci:workflow_test")
    seed_expression = "set(%s)" % " ".join(sorted(seeds))
    affected = f'kind("rule", rdeps(//..., {seed_expression}))'
    build_targets = query(affected, root, bazel, runner)
    test_targets = query(
        f'tests({affected}) except attr("tags", "requires-qemu", //...)',
        root,
        bazel,
        runner,
    )
    return build_targets, test_targets


def write_targets(path, targets):
    Path(path).write_text("".join(f"{target}\n" for target in targets))


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--build-output", required=True)
    parser.add_argument("--test-output", required=True)
    parser.add_argument("--bazel", default="bazel")
    args = parser.parse_args(argv)
    root = Path.cwd().resolve()
    changes = changed_paths(args.base, args.head, root)
    build_targets, test_targets = compute_targets(changes, root, args.bazel)
    write_targets(args.build_output, build_targets)
    write_targets(args.test_output, test_targets)
    print(
        f"Selected {len(build_targets)} build targets and "
        f"{len(test_targets)} non-E2E test targets from {len(changes)} changed paths.",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
