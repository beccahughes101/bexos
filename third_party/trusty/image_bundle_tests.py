import io
from pathlib import Path
import tarfile
import tempfile
import unittest
import subprocess
import sys
import struct
from unittest.mock import patch

import image_bundle as bundle


def artifact(name, architecture="aarch64"):
    if name in ("lk.elf", "boot.elf"):
        data = bytearray(64)
        data[:7] = b"\x7fELF" + bytes([1 if name == "boot.elf" else 2, 1, 1])
        machine = 3 if name == "boot.elf" else (183 if architecture == "aarch64" else 62)
        struct.pack_into("<HH", data, 16, 3 if architecture == "aarch64" else 2, machine)
        return data
    return b"firmware-" + name.encode()


class BundleTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.sources = []
        for name in bundle.ARTIFACTS:
            source = self.root / name
            source.write_bytes(artifact(name))
            self.sources.append(source)
        self.image = self.root / "image.bin"
        bundle.write_bundle(self.image, self.sources)

    def test_x86_bundle_separates_architecture_and_variant(self):
        sources = []
        for name in bundle.X86_ARTIFACTS:
            source = self.root / name
            source.write_bytes(artifact(name, "x86_64"))
            sources.append(source)
        bundle.write_bundle(self.image, sources, architecture="x86_64")
        self.assertIn(b'architecture: "x86_64"', bundle.read_bundle(self.image, architecture="x86_64")[bundle.METADATA])
        with self.assertRaises(ValueError):
            bundle.read_bundle(self.image)
        with self.assertRaises(ValueError):
            bundle.read_bundle(self.image, "authmgr_acceptance", "x86_64")
        previous = self.image.read_bytes()
        sources[0].write_bytes(b"changed")
        with patch.object(bundle.os, "replace", side_effect=OSError("interrupted")):
            with self.assertRaises(OSError):
                bundle.write_bundle(self.image, sources, architecture="x86_64")
        self.assertEqual(self.image.read_bytes(), previous)

    def test_x86_rejects_unloadable_kernel_types(self):
        sources = []
        for name in bundle.X86_ARTIFACTS:
            source = self.root / name
            source.write_bytes(artifact(name, "x86_64"))
            sources.append(source)
        for elf_type in (1, 3):
            with self.subTest(elf_type=elf_type):
                data = bytearray(artifact("lk.elf", "x86_64"))
                struct.pack_into("<H", data, 16, elf_type)
                (self.root / "lk.elf").write_bytes(data)
                with self.assertRaisesRegex(ValueError, "ELF architecture"):
                    bundle.write_bundle(self.image, sources, architecture="x86_64")

    def test_elf_architecture_mismatch_preserves_bundle(self):
        previous = self.image.read_bytes()
        (self.root / "lk.elf").write_bytes(artifact("lk.elf", "x86_64"))
        with self.assertRaisesRegex(ValueError, "ELF architecture"):
            bundle.write_bundle(self.image, self.sources)
        self.assertEqual(self.image.read_bytes(), previous)

    def test_round_trip_and_explicit_reuse(self):
        previous = self.image.read_bytes()
        self.sources[0].write_bytes(b"changed source")
        output = self.root / "extracted"
        bundle.extract_bundle(output, self.image)
        self.assertEqual((output / "bl1.bin").read_bytes(), b"firmware-bl1.bin")
        self.assertEqual((output / "rpmb_dev").stat().st_mode & 0o777, 0o755)
        self.assertEqual(self.image.read_bytes(), previous)
        bundle.write_bundle(self.image, self.sources)
        self.assertEqual(bundle.read_bundle(self.image)["bl1.bin"], b"changed source")

    def test_failed_refresh_preserves_previous_bundle(self):
        previous = self.image.read_bytes()
        with patch.object(bundle.os, "replace", side_effect=OSError("interrupted")):
            with self.assertRaises(OSError):
                bundle.write_bundle(self.image, self.sources)
        self.assertEqual(self.image.read_bytes(), previous)
        self.assertEqual(set(p.name for p in self.root.iterdir()),
                         set((*bundle.ARTIFACTS, "image.bin")))

    def test_oversized_secure_image_does_not_replace_previous_bundle(self):
        previous = self.image.read_bytes()
        with patch.object(bundle, "MAX_TRUSTY_IMAGE", 1):
            with self.assertRaisesRegex(ValueError, "secure RAM"):
                bundle.write_bundle(self.image, self.sources)
        self.assertEqual(self.image.read_bytes(), previous)

    def test_missing_corrupt_and_wrong_variant(self):
        with self.assertRaises(FileNotFoundError):
            bundle.read_bundle(self.root / "missing")
        with self.assertRaises(ValueError):
            bundle.read_bundle(self.image, "authmgr_acceptance")
        data = self.image.read_bytes().replace(b"firmware-bl1.bin", b"tampered-bl1.bin")
        self.image.write_bytes(data)
        with self.assertRaises(ValueError):
            bundle.read_bundle(self.image)

    def test_cli_missing_and_corrupt_bundles_name_the_matching_refresh_target(self):
        for variant, target in (("standard", "refresh_image"),
                                ("authmgr_acceptance", "refresh_authmgr_acceptance_image")):
            for sources in ([], [str(self.root / "corrupt.bin")]):
                (self.root / "corrupt.bin").write_bytes(b"not a firmware bundle")
                output = self.root / "not-extracted"
                result = subprocess.run(
                    [sys.executable, bundle.__file__, "--variant", variant,
                     "extract", str(output), *sources], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"bazel run //third_party/trusty:{target}", result.stderr)
                self.assertFalse(output.exists())

    def test_unsafe_duplicate_and_missing_members(self):
        for name in ("../escaped", "bl1.bin"):
            with tarfile.open(self.image, "a") as archive:
                member = tarfile.TarInfo(name)
                member.size = 1
                archive.addfile(member, io.BytesIO(b"x"))
            with self.assertRaises(ValueError):
                bundle.extract_bundle(self.root / "out", self.image)
            bundle.write_bundle(self.image, self.sources)
        with tarfile.open(self.image, "w"):
            pass
        with self.assertRaises(ValueError):
            bundle.read_bundle(self.image)


if __name__ == "__main__":
    unittest.main()
