"""Reject an E2E preflight that executed a non-test action outside the cache."""

import argparse
import json
from pathlib import Path


def entries(text):
    decoder = json.JSONDecoder()
    index = 0
    while index < len(text):
        while index < len(text) and text[index].isspace():
            index += 1
        if index == len(text):
            return
        entry, index = decoder.raw_decode(text, index)
        yield entry


def cache_misses(text):
    return [
        entry
        for entry in entries(text)
        if entry.get("mnemonic") != "TestRunner" and not entry.get("cacheHit", False)
    ]


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("execution_log")
    args = parser.parse_args(argv)
    misses = cache_misses(Path(args.execution_log).read_text())
    if misses:
        labels = ", ".join(
            "%s (%s)" % (entry.get("targetLabel", "unknown target"), entry.get("mnemonic", "unknown action"))
            for entry in misses
        )
        raise SystemExit(f"E2E cache handoff executed non-test actions: {labels}")


if __name__ == "__main__":
    main()
