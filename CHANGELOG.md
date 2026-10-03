# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado service list --unowned` and `stado service reap` also find processes whose working directory is under a managed root (on macOS through `lsof -d cwd`, on Linux through `/proc/<pid>/cwd`), not only processes whose program or entry point is a path under it. A process a job started with relative paths (`bash release/stado-build.sh`, `node node_modules/playwright-core/cli.js install ffmpeg`) named no such path, so it was never listed or reaped, and a reaped download was started again by an installer nobody could see.
- `stado product catalog --json` serves every product record whole. It served a fixed projection (id, name, family, description, surfaces, installations), so the `rivals`, `benchmark` and `roadmap` a record declares never reached `probierz benchmark rivals`, which refused every product with `the catalog names no rival for <product>`.
