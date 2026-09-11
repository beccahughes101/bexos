#!/usr/bin/env python3
import argparse
import binascii
import os
import struct
import sys
import uuid

SECTOR = 512
PARTITION_ENTRY_COUNT = 128
PARTITION_ENTRY_SIZE = 128
PARTITION_ARRAY_SECTORS = (PARTITION_ENTRY_COUNT * PARTITION_ENTRY_SIZE) // SECTOR
DISK_GUID = uuid.UUID("11111111-2222-3333-4444-555555555555")
BASIC_DATA_GUID = uuid.UUID("ebd0a0a2-b9e5-4433-87c0-68b6b72699c7")
ESP_GUID = uuid.UUID("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")

PARTITIONS = [
    ("ESP", ESP_GUID, 1024),
    ("BOOT_A", BASIC_DATA_GUID, 2048),
    ("BOOT_B", BASIC_DATA_GUID, 2048),
    ("SYS_STATE", BASIC_DATA_GUID, 65536),
    # Match qemu_storage in device/virtual/qemu/base/images.bzl (256 MiB).
    ("STORAGE", BASIC_DATA_GUID, 524288),
]


def guid_bytes(value):
    return value.bytes_le


def label_bytes(label):
    return label.encode("utf-16le")[:72].ljust(72, b"\0")


def crc32(data):
    return binascii.crc32(data) & 0xFFFFFFFF


def partition_array(partitions):
    entries = bytearray(PARTITION_ENTRY_COUNT * PARTITION_ENTRY_SIZE)
    for index, part in enumerate(partitions):
        first_lba, last_lba, name, type_guid = part
        unique = uuid.uuid5(uuid.NAMESPACE_DNS, f"bexos-qemu-aarch64-{name}")
        offset = index * PARTITION_ENTRY_SIZE
        entries[offset : offset + PARTITION_ENTRY_SIZE] = struct.pack(
            "<16s16sQQQ72s",
            guid_bytes(type_guid),
            guid_bytes(unique),
            first_lba,
            last_lba,
            0,
            label_bytes(name),
        )
    return bytes(entries)


def header(current_lba, backup_lba, first_usable, last_usable, partition_entries_lba, array_crc):
    raw = bytearray(SECTOR)
    struct.pack_into(
        "<8sIIIIQQQQ16sQIII",
        raw,
        0,
        b"EFI PART",
        0x00010000,
        92,
        0,
        0,
        current_lba,
        backup_lba,
        first_usable,
        last_usable,
        guid_bytes(DISK_GUID),
        partition_entries_lba,
        PARTITION_ENTRY_COUNT,
        PARTITION_ENTRY_SIZE,
        array_crc,
    )
    struct.pack_into("<I", raw, 16, crc32(raw[:92]))
    return bytes(raw)


def build_image(partition_payloads=None):
    partition_payloads = partition_payloads or {}
    first_usable = 34
    cursor = 2048
    partitions = []
    for name, type_guid, sectors in PARTITIONS:
        first = cursor
        last = first + sectors - 1
        partitions.append((first, last, name, type_guid))
        cursor = last + 1

    total_sectors = cursor + PARTITION_ARRAY_SECTORS + 34
    last_lba = total_sectors - 1
    last_usable = last_lba - PARTITION_ARRAY_SECTORS - 1

    primary_array = partition_array(partitions)
    array_crc = crc32(primary_array)
    image = bytearray(total_sectors * SECTOR)

    image[0:SECTOR] = protective_mbr(last_lba)
    image[SECTOR : 2 * SECTOR] = header(1, last_lba, first_usable, last_usable, 2, array_crc)
    image[2 * SECTOR : (2 + PARTITION_ARRAY_SECTORS) * SECTOR] = primary_array

    backup_array_lba = last_lba - PARTITION_ARRAY_SECTORS
    image[backup_array_lba * SECTOR : last_lba * SECTOR] = primary_array
    image[last_lba * SECTOR : (last_lba + 1) * SECTOR] = header(
        last_lba, 1, first_usable, last_usable, backup_array_lba, array_crc
    )
    for first, last, name, _ in partitions:
        payload = partition_payloads.get(name)
        if payload is None:
            continue
        capacity = (last - first + 1) * SECTOR
        if len(payload) > capacity:
            raise ValueError(f"partition payload {name} is {len(payload)} bytes, capacity is {capacity}")
        image[first * SECTOR : first * SECTOR + len(payload)] = payload
    return bytes(image)


def protective_mbr(last_lba):
    raw = bytearray(SECTOR)
    size = min(last_lba, 0xFFFFFFFF)
    raw[446 : 446 + 16] = struct.pack("<B3sB3sII", 0, b"\0\2\0", 0xEE, b"\xff\xff\xff", 1, size)
    raw[510:512] = b"\x55\xaa"
    return bytes(raw)


def self_test():
    image = build_image()
    assert image[510:512] == b"\x55\xaa"
    assert image[SECTOR : SECTOR + 8] == b"EFI PART"
    assert image[-SECTOR : -SECTOR + 8] == b"EFI PART"
    entries_lba = struct.unpack_from("<Q", image, SECTOR + 72)[0]
    first_name = image[entries_lba * SECTOR + 56 : entries_lba * SECTOR + 128]
    assert first_name.startswith("ESP".encode("utf-16le"))
    assert len(image) > 32 * 1024 * 1024
    embedded = build_image({"SYS_STATE": b"BEXFS-test"})
    sys_state_first = 2048 + 1024 + 2048 + 2048
    assert embedded[sys_state_first * SECTOR : sys_state_first * SECTOR + 10] == b"BEXFS-test"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out")
    parser.add_argument("--partition-image", action="append", default=[])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0
    if not args.out:
        parser.error("--out is required")

    payloads = {}
    for spec in args.partition_image:
        if "=" not in spec:
            parser.error("--partition-image requires LABEL=path")
        label, path = spec.split("=", 1)
        if label in payloads:
            parser.error(f"duplicate partition payload for {label}")
        if label not in {partition[0] for partition in PARTITIONS}:
            parser.error(f"unknown partition label {label}")
        with open(path, "rb") as source:
            payloads[label] = source.read()
    try:
        image = build_image(payloads)
    except ValueError as error:
        parser.error(str(error))
    with open(args.out, "wb") as handle:
        handle.write(image)
    return 0


if __name__ == "__main__":
    sys.exit(main())
