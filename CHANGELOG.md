# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.60](changelog/0.23.42-0.23.60.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **The janitor keeps the release pipeline's own records:** the `object_evidence` cleaner takes every file under the fleet store's `probierz/runs` prefix at the disk-full threshold, and that prefix also holds every release run (`runs/release-pipeline`), build record (`runs/build`) and change batch (`runs/release-changes`). On charless-mac-mini, the host serving the fleet store, it deleted them mid-delivery: `stado build status` answered `build … does not exist` for a build that had passed minutes before and `stado release status stado` answered `unknown release product` (f3e89522). Those three directories are now kept, reported as `stado_record_or_pinned_input_kept` with the pinned `native-signing` inputs; product run evidence is still taken.

- **The janitor takes the compiler cache's store:** every Cargo build Stado runs compiles through Kache, whose store (`~/Library/Caches/kache` on macOS, `~/.cache/kache` on Linux) only grows, and no cleaner covered it — `stado space report` read it uncovered at 19.2 GiB on charless-mac-mini and 5.2 GiB on ubuntu-server-rtx-pro-6000 while both sat at the disk-full threshold. The new `compiler_cache` cleaner removes every file of the store at the threshold, except while a job runs (`job_running`), since a running build reads and writes it; what it costs is a slower next build.

- **The same installation asked twice waits instead of restarting:** `stado product install` and `update` stop an older installation of the same surface that has placed nothing yet, so a newer request replaces it. When two sessions asked for the same thing on one host — `stado product install stado --surface cli` and the same with `--json` — each stopped the other's build as soon as it started, neither ever placed a file, and the host kept crash-looping on the Stado it was trying to replace. A holder whose recorded command asks for the same installation (the same arguments after the program path, `--json` aside) is now waited for, printed as `waiting for <lock>: held by pid <pid> … (the same installation, already under way)`; a different request still supersedes it.
