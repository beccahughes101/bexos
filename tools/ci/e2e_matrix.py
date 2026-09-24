"""Generate the maintained one-target-per-runner E2E matrix."""

import argparse
import json
from pathlib import Path
import subprocess


SUITES = (
    "//testing/e2e/qemu:aarch64",
    "//testing/e2e/qemu:x86_64",
    "//testing/e2e/qemu:x86_64_development",
    "//testing/e2e/qemu:firmware_acceptance",
)


def run_query(bazel="bazel"):
    expression = "tests(set(%s))" % " ".join(SUITES)
    result = subprocess.run(
        [bazel, "query", expression, "--output=label"],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    return result.stdout.splitlines()


def architecture(label):
    target = label.rsplit(":", 1)[-1]
    if target.startswith("x86_64_") or target.endswith("_x86_64"):
        return "x86_64"
    return "aarch64"


def make_matrix(labels):
    unique = sorted({label for label in labels if label.startswith("//")})
    if len(unique) > 256:
        raise ValueError(f"E2E matrix has {len(unique)} jobs; GitHub permits at most 256")
    return {
        "include": [
            {
                "architecture": architecture(label),
                "id": f"{index:03d}",
                "label": label,
            }
            for index, label in enumerate(unique, 1)
        ]
    }


def encode_matrix(labels):
    return json.dumps(make_matrix(labels), separators=(",", ":"), sort_keys=True)


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--bazel", default="bazel")
    parser.add_argument("--github-output")
    args = parser.parse_args(argv)
    encoded = encode_matrix(run_query(args.bazel))
    if args.github_output:
        with Path(args.github_output).open("a") as output:
            output.write(f"matrix={encoded}\n")
    else:
        print(encoded)


if __name__ == "__main__":
    main()
