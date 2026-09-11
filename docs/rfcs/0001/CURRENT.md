# RFC 0001: RFC structure and authoring — current implementation

- Reviewed: 2026-09-10
- Repository revision: `7638e4ce9bad51c7aee4737853da41569593cff2` (implementation baseline; documentation changes in this update are included).
- Design: [RFC 0001](README.md)

## Implementation summary

The repository uses numbered RFC directories and now pairs every design with an implementation snapshot. Detailed implementation guides live directly under `docs`.

## Implemented behavior

- RFCs 0001–0061 each have a design README, review metadata, an implementation/gaps snapshot, and reciprocal navigation. The RFC index links both documents.
- RFC 0001 defines authoring, numbering, evidence, preservation of future designs, and maintenance of CURRENT.md when implementation changes.
- The 31 former current-documentation files, including four JSON reports, are relocated without filename changes; the CLI reference remains a Bazel test input.

## Gaps and deviations

- Implementation snapshots are dated audits, not automatic status reporting or evidence that the operating system test suite passed.
- Older implementation notes inside RFCs and historical validation guides may describe earlier states; this companion records the reviewed baseline. Keeping it accurate requires updates alongside code changes.

## Sources and validation

Implementation and contract evidence: [docs/rfcs/0001/README.md](../../../docs/rfcs/0001/README.md), [docs/rfcs/README.md](../../../docs/rfcs/README.md), [docs/README.md](../../../docs/README.md), [BUILD.bazel](../../../BUILD.bazel).

Relevant test sources and Bazel targets: [tools/bexctl/BUILD.bazel](../../../tools/bexctl/BUILD.bazel).

Checks run for this documentation update:

- Coverage, review metadata, and reciprocal/index navigation for all 61 RFCs.
- Relative documentation/source links and Markdown structure.
- Preservation of all 31 moved files, including byte-identical JSON reports.
- `git diff --check`.
- `bazel test //tools/bexctl:bexctl_tests`: passed (8 tests in one target).
- `bazel run @rules_rust//:rustfmt`: passed; no Rust source changes.

No operating-system runtime, hardware, or performance suite was run. Other RFC
companions identify inspected test sources and historical evidence without
claiming those subsystem suites were rerun.
