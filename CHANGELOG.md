# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.63](changelog/0.23.42-0.23.63.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado release submit --source` runs the commit's quality check before it claims the version:** the claim is immutable, and a commit whose `fmt` gate refused it on the builder spent the version for nothing — Stado 0.23.52 and 0.23.63 were lost that way, and every retry needed a new number. The submission now runs `stado quality check` on the exported commit (lock resolution, the declared formatting gates, pinned inputs present) first; a refusal prints the gate's report, e.g. `gate "fmt" of stado refuses 68e553af… cargo fmt … --check exited 1; stado quality format writes what it reads`, and nothing is claimed, uploaded or queued.
