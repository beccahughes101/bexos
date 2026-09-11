# RFC 0001: RFC structure and authoring

- Created: 2026-09-10
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

An RFC records one BexOS design: the problem it addresses, the proposed behavior,
and the reasoning and constraints behind it. RFCs use a numbered directory and a
lightweight Markdown structure, with topic-specific sections that suit the design.
The [RFC index](../README.md) lists the available documents.

RFCs retain the full long-term design in `README.md`. Each RFC also has a
`CURRENT.md` companion describing implemented behavior, gaps against the design,
and supporting evidence. [Implementation guides](../../README.md) live directly
under `docs` and provide detailed operational and subsystem documentation. An RFC
number does not imply approval, implementation, or successful validation.

## Location and numbering

- Store each RFC at `docs/rfcs/NNNN/README.md`, with a four-digit, zero-padded number,
  and its implementation snapshot at `docs/rfcs/NNNN/CURRENT.md`.
- RFC `0001` describes this structure. RFCs `0002`–`0061` migrate the 60 design
  documents present when the RFC directory was established.
- The migration orders designs by the author timestamp of their earliest Git
  addition, following renames. Equal timestamps are ordered alphabetically by
  original source path. These timestamps record the earliest available repository
  evidence, not an inferred date of authorship outside Git.
- New RFCs take the next unused number after the highest existing RFC number.
  After this migration, that number is `0062`. Do not backfill, reuse, or renumber
  published RFCs when an older design is discovered.
- Add design and current-implementation links to the index when adding an RFC.
  Link each README to its CURRENT companion and each companion back to its design.
  Overlapping designs may have
  separate RFCs; explain their relationship with links rather than silently
  merging or replacing them.

## Required structure

Start with a single level-one heading, `RFC NNNN: Descriptive title`, followed by
the creation date. Use an ISO date (`YYYY-MM-DD`) for a new RFC. Migrated RFCs retain
the full Git author timestamp, including its offset, so chronological ordering is
auditable.

Migrated RFCs retain the design's contents at migration time, including additions
made after its initial creation.

Follow the metadata with `## Summary`: a short paragraph that describes the design
and its purpose. Then use descriptive level-two sections for the topic, with
level-three and deeper headings only for subsections. Do not skip heading levels.
Number steps within procedures when order matters; section headings do not need
numeric prefixes.

Use this starting template, replacing the title, date, and prose:

````markdown
# RFC NNNN: Descriptive title

- Created: YYYY-MM-DD
- Current implementation: [Implementation and gaps](CURRENT.md)

## Summary

Describe the design and the problem it addresses in a short paragraph.

## Design overview

Explain the context, goals, and principal responsibilities.

## Topic-specific section

Describe the relevant behavior, interfaces, constraints, and rationale.

### A related detail

Use subsections when they clarify the relationship between ideas.
````

For a migration, use the full timestamp in the date line:

```text
- Created: YYYY-MM-DDTHH:MM:SS±HH:MM
```

## Choosing sections

Organize the document around the design rather than filling a fixed checklist.
Useful sections include architecture, lifecycle, interfaces, security boundaries,
failure handling, alternatives, implementation notes, validation, and future work.
Include them where they carry substantive content; do not invent alternatives or
open questions just to populate headings.

- Explain ownership and data flow before detailed interfaces or examples.
- Keep requirements, rationale, tradeoffs, and alternatives together where they
  explain a decision. Preserve unresolved choices as unresolved choices.
- Keep implementation notes distinct from proposed behavior. Retain their dates,
  evidence, caveats, and links to current documentation.
- Keep deferred features and long-term designs when updating implementation
  status. Update the relevant `CURRENT.md` and implementation guides in `docs`
  when implemented behavior changes; do not erase future design from the RFC to
  make it match the implementation.
- Label sketches, proposed interfaces, estimates, and targets appropriately.
  Do not present unmeasured performance or unvalidated hardware behavior as an
  accepted implementation result.

## Writing and examples

Use direct technical prose. Replace conversational replies, praise, rhetorical
questions, and promotional introductions with statements of the design and its
rationale. Remove repetition only when it carries no distinct information.

Editorial cleanup must preserve technical intent: requirements, defaults,
identifiers, numeric values, error behavior, security boundaries, alternatives,
and future work. Preserve the difference between “must,” “should,” and “may.”
If a source contains conflicting designs or status claims, retain the distinction;
resolving the architecture is a separate design change.

Use lists for parallel properties and ordered procedures, tables for comparisons,
and fenced blocks for code and diagrams. Nest supporting list items under the
item they describe. Give code fences a language where it is known, and keep
technical examples intact during prose edits. Remove stray AI citation markers
and repair accidental Markdown inside code without changing example semantics.

App manifests and configuration use prototxt. Builds and code generation use
Bazel, and generated artifacts are not committed. A historical example is not
silently redesigned during migration to conform to a newer convention.

## Links and supporting material

- Link to another RFC using `../NNNN/README.md`, optionally with a section anchor.
- Link to another RFC's implementation snapshot using `../NNNN/CURRENT.md`.
- Link to an implementation guide using `../../example.md`.
- From a CURRENT companion, link to repository sources using `../../../path`.
- Use descriptive link text and update affected anchors when renaming headings.
- Keep supporting material in the RFC directory where needed and link it with
  relative paths. Repository code identifiers and Bazel labels remain literal.

The original designs were migrated to RFCs and the old `docs/design` directory
was subsequently removed. Design references now point to RFCs. Implementation
guides and their supporting reports live directly in `docs`; do not recreate a
separate current-documentation directory.

## Current implementation companion

Every RFC, including this authoring RFC and designs without a runtime
implementation, requires a substantive `CURRENT.md`. It is a self-contained,
dated implementation snapshot, not a replacement for either the design or the
detailed guides. Review the relevant code, interfaces, package manifests,
product configuration, and tests rather than copying old status statements.

- Record the review date and full repository revision. State whether the review
  includes uncommitted changes; never attribute older test results to a newer tree.
- Explain the implemented paths, owners, interfaces, and product integration.
  Distinguish a schema, library/model, build smoke target, and deployed runtime.
- Describe missing capabilities, partial behavior, and deviations from the RFC.
  If no implementation is found, say which relevant surfaces were inspected and
  distinguish related foundations from the missing feature.
- For services and drivers, describe heart transplant support and its limits.
- Link concrete source/configuration evidence and relevant test sources or Bazel
  targets. Separate tests inspected, historical results, and checks actually run.
  Unverified hardware, performance, and runtime behavior must remain unverified.
- Keep historical notes and future designs in the README. When an older status
  note is stale, explain the newer state and the difference in CURRENT.md.
- Update the companion and affected guides alongside implementation changes.
  Link overlapping RFCs rather than treating one subsystem's completion as proof
  that every related design is complete.

Use this companion template, replacing all illustrative values and prose:

```markdown
# RFC NNNN: Descriptive title — current implementation

- Reviewed: YYYY-MM-DD
- Repository revision: FULL_GIT_REVISION (state any working-tree additions).
- Design: [RFC NNNN](README.md)

## Implementation summary

Describe what exists and how much of the design it implements.

## Implemented behavior

Describe concrete runtime behavior, interfaces, product wiring, and lifecycle.

## Gaps and deviations

Describe missing or partial behavior and differences from the design.

## Sources and validation

Link implementation evidence, relevant tests, and detailed guides. State which
checks were actually run and retain the scope/date of historical results.
```

## Review before publication

Check the RFC number, metadata, index entries for both documents, reciprocal
links, heading hierarchy, local links, and Markdown rendering. Check the
companion's evidence, gaps, review revision, and validation boundaries.
For a migrated design, compare the complete RFC with its
source, including examples, tables, diagrams, limitations, and future work.
Verify that every source has exactly one RFC and that the originals are unchanged.
Documentation-only migrations use documentation checks; they do not require the
unrelated operating-system test suite.
