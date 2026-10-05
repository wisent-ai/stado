# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.28](changelog/0.23.12-0.23.28.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **An install's script step finds the toolchain too, and a script that could not find its program no longer condemns the revision (76c2079f):** `stado product install` resolved a step's own program in `~/.stado/bin`, `~/.local/bin`, `~/.cargo/bin`, `/opt/homebrew/bin` and `/usr/local/bin` but handed the step the host agent's minimal PATH, so Oko's `bash release/quality.sh` exited 127 on the `cargo` it calls, and that exit was recorded as the revision failing its gate: every later install of that Oko revision was refused with `already failed quality … commit a repair`, and every Tama release failed installing Oko for its post-build test. An install step's PATH is now those directories ahead of the inherited one, as the release worker's already was; a step that exits 126 or 127 (the shell could not find or run a program) is not recorded; and records written before this release, which counted such exits, are recorded as `gate-verdict` and no longer refuse, while new ones read `tree-verdict`.
