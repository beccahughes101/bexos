#!/usr/bin/env python3
"""Append the authenticated live-replacement manifest to an ARM Trusty ELF."""

from pathlib import Path
import re
import struct
import sys


MAGIC = b"BEXARM01"
FIELDS = {
    "architecture": "string",
    "generation": "uint",
    "migration_abi": "uint",
    "fixture": "string",
}
FIXTURES = {
    "standard": 0,
    "incompatible_state": 1,
    "fault": 2,
    "hang": 3,
}


def parse_config(path: Path) -> dict[str, object]:
    config: dict[str, object] = {}
    for number, raw_line in enumerate(path.read_text().splitlines(), 1):
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        match = re.fullmatch(r'([a-z_]+):\s*(?:"([^"]*)"|([0-9]+))', line)
        if not match:
            raise SystemExit(f"{path}:{number}: invalid Trusty image config line")
        key, quoted, integer = match.groups()
        if key not in FIELDS:
            raise SystemExit(f"{path}:{number}: unknown Trusty image config field {key}")
        if key in config:
            raise SystemExit(f"{path}:{number}: duplicate Trusty image config field {key}")
        if FIELDS[key] == "string":
            if quoted is None:
                raise SystemExit(f"{path}:{number}: {key} requires a quoted string")
            config[key] = quoted
        else:
            if integer is None:
                raise SystemExit(f"{path}:{number}: {key} requires an unsigned integer")
            value = int(integer)
            if value == 0 or value >= 2**63:
                raise SystemExit(f"{path}:{number}: invalid {key}")
            config[key] = value
    missing = FIELDS.keys() - config.keys()
    if missing:
        raise SystemExit(f"{path}: missing Trusty image config fields: {', '.join(sorted(missing))}")
    return config


def main() -> None:
    if len(sys.argv) != 5:
        raise SystemExit(
            "usage: decorate_arm_candidate.py INPUT_ELF CONFIG EXPECTED_GENERATION OUTPUT_ELF"
        )
    source = Path(sys.argv[1])
    config_path = Path(sys.argv[2])
    expected_generation = int(sys.argv[3])
    output = Path(sys.argv[4])
    config = parse_config(config_path)
    if config["architecture"] != "aarch64":
        raise SystemExit(f"{config_path}: ARM candidate must declare architecture \"aarch64\"")
    if config["generation"] != expected_generation:
        raise SystemExit(
            f"{config_path}: generation {config['generation']} does not match signed generation "
            f"{expected_generation}"
        )
    fixture = config["fixture"]
    if fixture not in FIXTURES:
        raise SystemExit(f"{config_path}: invalid Trusty fixture {fixture!r}")
    image = source.read_bytes()
    if not image.startswith(b"\x7fELF\x02\x01\x01"):
        raise SystemExit(f"{source}: candidate is not a little-endian ELF64 image")
    if MAGIC in image:
        raise SystemExit(f"{source}: candidate already contains an ARM replacement manifest")
    manifest = MAGIC + struct.pack(
        "<QQQ",
        expected_generation,
        config["migration_abi"],
        FIXTURES[fixture],
    )
    output.write_bytes(image + manifest)


if __name__ == "__main__":
    main()
