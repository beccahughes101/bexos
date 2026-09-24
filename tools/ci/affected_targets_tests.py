import tempfile
import unittest
from pathlib import Path

import affected_targets


class FakeRunner:
    def __init__(self, query_outputs=None, submodules=()):
        self.query_outputs = iter(query_outputs or ())
        self.submodules = set(submodules)
        self.commands = []

    def __call__(self, command, _cwd):
        self.commands.append(command)
        if command[:3] == ["git", "ls-files", "--stage"]:
            return "160000 abc 0\t%s\n" % command[-1] if command[-1] in self.submodules else ""
        if command[:2] == ["bazel", "query"]:
            return next(self.query_outputs)
        raise AssertionError(command)


class AffectedTargetsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "BUILD.bazel").write_text("")
        (self.root / "lib/example").mkdir(parents=True)
        (self.root / "lib/example/BUILD.bazel").write_text("")

    def tearDown(self):
        self.temporary.cleanup()

    def test_package_pattern_uses_nearest_package(self):
        self.assertEqual(
            affected_targets.package_pattern("lib/example/src/lib.rs", self.root),
            "//lib/example:*",
        )
        self.assertEqual(affected_targets.package_pattern("docs/ci.md", self.root), "//:*")

    def test_global_changes_and_deletions_fall_back(self):
        for change in [[("M", "MODULE.bazel")], [("M", "build/rule.bzl")], [("D", "lib/a.rs")]]:
            runner = FakeRunner()
            self.assertEqual(
                affected_targets.compute_targets(change, self.root, runner=runner),
                (["//..."], ["//..."]),
            )

    def test_submodule_change_falls_back(self):
        runner = FakeRunner(submodules={"third_party/repo"})
        self.assertEqual(
            affected_targets.compute_targets(
                [("M", "third_party/repo")], self.root, runner=runner
            ),
            (["//..."], ["//..."]),
        )

    def test_empty_change_is_empty(self):
        self.assertEqual(affected_targets.compute_targets([], self.root), ([], []))

    def test_docs_only_change_is_empty(self):
        runner = FakeRunner()
        self.assertEqual(
            affected_targets.compute_targets(
                [("M", "docs/ci.md"), ("M", "docs/testing-status.md")],
                self.root,
                runner=runner,
            ),
            ([], []),
        )

    def test_queries_reverse_dependencies_and_excludes_e2e_tests(self):
        runner = FakeRunner(
            query_outputs=[
                "//lib/example:example\n//services/example:service\n",
                "//lib/example:example_tests\n",
            ]
        )
        build, tests = affected_targets.compute_targets(
            [("M", "lib/example/src/lib.rs")], self.root, runner=runner
        )
        self.assertEqual(build, ["//lib/example:example", "//services/example:service"])
        self.assertEqual(tests, ["//lib/example:example_tests"])
        self.assertIn("rdeps(//..., set(//lib/example:*))", runner.commands[-2][2])
        self.assertIn('except attr("tags", "requires-qemu", //...)', runner.commands[-1][2])

    def test_ci_change_always_selects_workflow_test(self):
        (self.root / ".github/workflows").mkdir(parents=True)
        (self.root / ".github/BUILD.bazel").write_text("")
        runner = FakeRunner(query_outputs=["//tools/ci:workflow_test\n", "//tools/ci:workflow_test\n"])
        affected_targets.compute_targets(
            [("M", ".github/workflows/ci.yml")], self.root, runner=runner
        )
        self.assertIn("//tools/ci:workflow_test", runner.commands[-2][2])


if __name__ == "__main__":
    unittest.main()
