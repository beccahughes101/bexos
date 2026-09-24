import json
import unittest

import e2e_matrix


class E2eMatrixTests(unittest.TestCase):
    def test_matrix_is_sorted_deduplicated_and_architected(self):
        encoded = e2e_matrix.encode_matrix(
            [
                "//z:test_x86_64",
                "//firmware:x86_64_acceptance_test",
                "//a:firmware_test",
                "//z:test_x86_64",
                "noise",
            ]
        )
        self.assertEqual(
            json.loads(encoded),
            {
                "include": [
                    {"architecture": "aarch64", "id": "001", "label": "//a:firmware_test"},
                    {
                        "architecture": "x86_64",
                        "id": "002",
                        "label": "//firmware:x86_64_acceptance_test",
                    },
                    {"architecture": "x86_64", "id": "003", "label": "//z:test_x86_64"},
                ]
            },
        )
        self.assertEqual(
            encoded,
            e2e_matrix.encode_matrix(
                reversed(
                    [
                        "//z:test_x86_64",
                        "//a:firmware_test",
                        "//firmware:x86_64_acceptance_test",
                    ]
                )
            ),
        )

    def test_matrix_limit_is_enforced(self):
        with self.assertRaisesRegex(ValueError, "256"):
            e2e_matrix.make_matrix(f"//tests:test_{index}" for index in range(257))


if __name__ == "__main__":
    unittest.main()
