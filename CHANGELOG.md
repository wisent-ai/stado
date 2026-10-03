# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.14](changelog/0.23.12-0.23.14.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado release status` no longer reports a build job as lost while its builder runs it. In-flight jobs were looked for under running before queue, so a job claimed between the two reads was under neither and the run failed with `the job was lost … submit the release again`. The lookup now follows the lifecycle (queue, running, completed, uploaded, failed, cancelled) and walks it twice before calling a job lost, because a move fences its source before it writes the destination.

- `stado product install|update` names what occupies a canonical checkout path it cannot use: not a directory, a directory with no Git checkout, a checkout with no origin, or the origin it declares. It said only `<path> exists and does not identify <repository>`, which on a host reached through `--host` nobody could look at.
