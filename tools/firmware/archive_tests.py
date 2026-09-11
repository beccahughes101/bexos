import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import archive


class ArchiveTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.path = self.root / 'saved.bin'
        self.files = {'loader.efi': b'signed loader', 'monitor.elf': b'monitor'}
        self.header = 'format_version: 1\narchitecture: "x86_64"\nvariant: "standard"\n'
        archive.write(self.path, self.files, self.header, lambda _: None)

    def test_roundtrip_and_variant_binding(self):
        self.assertEqual(archive.read(self.path, tuple(self.files), self.header), self.files)
        with self.assertRaises(ValueError):
            archive.read(self.path, tuple(self.files), self.header.replace('standard', 'acceptance'))
        archive.extract(self.root / 'extracted', self.files, self.header)
        self.assertEqual((self.root / 'extracted/loader.efi').read_bytes(), self.files['loader.efi'])

    def rewrite(self, extra=None, missing=None, corrupt=False):
        with tarfile.open(self.path, 'w', format=tarfile.USTAR_FORMAT) as output:
            for name, data in [*self.files.items(), (archive.MANIFEST, archive.manifest(self.files, self.header))]:
                if name == missing:
                    continue
                if corrupt and name == 'loader.efi':
                    data = b'wrong loader!'
                member = tarfile.TarInfo(name)
                member.size = len(data)
                output.addfile(member, io.BytesIO(data))
            if extra is not None:
                output.addfile(extra)

    def test_modified_missing_duplicate_links_paths_and_oversized_members(self):
        self.rewrite(corrupt=True)
        with self.assertRaises(ValueError):
            archive.read(self.path, tuple(self.files), self.header)
        self.rewrite(missing='monitor.elf')
        with self.assertRaises(ValueError):
            archive.read(self.path, tuple(self.files), self.header)
        for name, kind, size in [('loader.efi', tarfile.REGTYPE, 0),
                                 ('../escape', tarfile.REGTYPE, 0),
                                 ('extra', tarfile.SYMTYPE, 0)]:
            member = tarfile.TarInfo(name)
            member.type = kind
            member.size = size
            self.rewrite(extra=member)
            with self.assertRaises(ValueError):
                archive.read(self.path, tuple(self.files), self.header)
        self.rewrite()
        with patch.object(archive, 'MAX_MEMBER', 4), self.assertRaises(ValueError):
            archive.read(self.path, tuple(self.files), self.header)

    def test_failed_validation_preserves_previous_saved_bundle(self):
        previous = self.path.read_bytes()
        def reject(_):
            raise ValueError('candidate firmware incompatible')
        with self.assertRaises(ValueError):
            archive.write(self.path, self.files, self.header, reject)
        self.assertEqual(self.path.read_bytes(), previous)
        self.assertEqual(list(self.root.iterdir()), [self.path])


if __name__ == '__main__':
    unittest.main()
