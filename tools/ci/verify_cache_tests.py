import json
import unittest

import verify_cache


def encoded(*entries):
    return "".join(json.dumps(entry) for entry in entries)


class VerifyCacheTests(unittest.TestCase):
    def test_allows_cache_hits_and_test_execution(self):
        text = encoded(
            {"cacheHit": True, "mnemonic": "Rustc", "targetLabel": "//lib:test"},
            {"cacheHit": False, "mnemonic": "TestRunner", "targetLabel": "//lib:test"},
        )
        self.assertEqual(verify_cache.cache_misses(text), [])

    def test_rejects_non_test_cache_miss(self):
        miss = {"cacheHit": False, "mnemonic": "Genrule", "targetLabel": "//image:disk"}
        self.assertEqual(verify_cache.cache_misses(encoded(miss)), [miss])


if __name__ == "__main__":
    unittest.main()
