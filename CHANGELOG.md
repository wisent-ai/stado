# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per range, because a file this repository cannot edit is
a file that stops receiving entries: the length gate refuses every write to a
file past 300 lines, and this one had reached 414. Two product fixes on
2026-09-08 could not be recorded at all until it was split.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.22.18](changelog/0.22.18-0.22.18.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- Stado carries no tests the operator has not approved with `tama tests approve`: the integration tests, the `stado-source-checks` package, Stado Desktop's test targets, the inline test modules, the recipe's `tests` stages and the post-build qualification in `deploy/release/build_stado.sh` are removed, and `stado release verify-platform`, which only ran those journeys, is gone from the CLI and Stado Desktop.
- A required platform that declares no post-build test qualifies on its passing build: `stado release changes` no longer leaves it `awaiting_tests`, `stado release catalog enroll` and `audit` no longer refuse it, and `build submit` no longer warns about it. A platform that declares tests still qualifies only on all of them passing.
- `stado release catalog adopt` writes no test stage and no `release/test.sh`, and `--kind npm` no longer requires `scripts.test`.
- `stado web smoke` is removed: starting a site and fetching a page is a smoke check, which does not count as a test.
