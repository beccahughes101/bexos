import unittest
from pathlib import Path
from layout import parse, generate

PROFILE = Path(__file__).with_name("qemu_aarch64.prototxt").read_text()

class LayoutTest(unittest.TestCase):
    def test_regions_fit_without_overlap_and_have_one_source_of_truth(self):
        values = parse(PROFILE)
        base = values["transition_base"]
        owner = values["owner_bytes"]
        bank = values["trusty_bank_bytes"]
        header, linker, rust = generate(values)
        for name, address in [("OWNER_BASE", base), ("TRUSTY_A_BASE", base + owner),
                              ("TRUSTY_B_BASE", base + owner + bank),
                              ("UPLOAD_BASE", base + owner + 2 * bank),
                              ("MIGRATION_BASE", base + owner + 2 * bank + values["upload_bytes"])]:
            self.assertIn(f"BEXOS_{name} 0x{address:x}", header)
            self.assertIn(f"BEXOS_{name} = 0x{address:x}", linker)
            self.assertIn(f"{name}: u64 = 0x{address:x}", rust)

    def test_reject_ambiguous_or_unsafe_memory_policy(self):
        for text in [PROFILE + "transition_base: 1099511627776", PROFILE + "typo: 4096",
                     PROFILE.replace("transition_base: 1099511627776", "transition_base: 0"),
                     PROFILE.replace("transition_base: 1099511627776", "transition_base: 281474976710656"),
                     PROFILE.replace("transition_bytes: 268435456", "transition_bytes: 67108864"),
                     PROFILE.replace("owner_bytes: 16777216", "owner_bytes: 4096"),
                     PROFILE.replace("owner_bytes: 16777216", "owner_bytes: 268435456"),
                     PROFILE.replace("upload_bytes: 68157440", "upload_bytes: 67108864"),
                     PROFILE.replace("migration_bytes: 8388608", "migration_bytes: 3"),
                     PROFILE.replace("migration_bytes: 8388608", "")]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                parse(text)

if __name__ == "__main__":
    unittest.main()
