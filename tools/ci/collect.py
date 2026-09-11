"""Collect diagnostics without invoking Bazel again after failure/cancellation."""
import os
from pathlib import Path
import shutil


def collect(workspace, temporary):
    destination = temporary / "bexos-ci-diagnostics"
    destination.mkdir(parents=True, exist_ok=True)
    logs = temporary / "bexos-ci-logs"
    if logs.is_dir():
        shutil.copytree(logs, destination / "commands", dirs_exist_ok=True)
    count = 0
    # Later bazel run commands can repoint bazel-testlogs to another build
    # configuration. Keep results from every configuration used by this job.
    roots = sorted((workspace / "bazel-out").glob("*/testlogs"))
    if not roots and (workspace / "bazel-testlogs").is_dir():
        roots = [workspace / "bazel-testlogs"]
    for testlogs in roots:
        configuration = testlogs.parent.name if testlogs.name == "testlogs" else "default"
        for source in testlogs.rglob("*"):
            relative = source.relative_to(testlogs)
            if source.is_file() and (
                source.name in ("test.log", "test.xml")
                or "test.outputs" in relative.parts
                or "test.outputs_manifest" in relative.parts
            ):
                target = destination / "tests" / configuration / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, target)
                count += 1
    (destination / "inventory.txt").write_text(
        f"Collected {count} test diagnostic files.\n"
        "No test files can mean that setup, analysis, or compilation failed before tests ran.\n"
    )


if __name__ == "__main__":
    collect(Path(os.environ["GITHUB_WORKSPACE"]), Path(os.environ["RUNNER_TEMP"]))
