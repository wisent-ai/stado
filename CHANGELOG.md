# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.22.19](changelog/0.22.18-0.22.19.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado_database::connect` opens a fleet SQLite database. `stado database place` now writes `pooler_url` (`sqlite://<file>`) into a SQLite item beside its host and path, and the library reads it without a certificate, since a file has no server to verify. Before, a SQLite database Stado created had no field any consumer could read. A missing file is refused naming the file and that it opens only on the host that holds it. `<PRODUCT>_DATABASE_URL=sqlite://…` needs no `_CA_FILE`.
- `stado product install|update --source-commit COMMIT` alone installs a source build of that exact canonical commit: the checkout exports that commit's tree, after proving `origin/main` carries it, instead of whatever head the checkout has advanced to. Before, every install built the newest `origin/main`, so a release worker could not be put on a commit that had already passed the fleet's gates while other work kept landing.
- A newer `stado product install|update` of a surface supersedes an older one still preparing: the holder is told to stop and the newer one takes the surface, as a newer fleet build cancels the builds it supersedes; a holder already placing files is waited for. `--wait` keeps the older one whatever it is doing. Before, the second install queued behind the first until it finished, including when the first was building a commit nobody wanted any more.
- `stado service logs|env|show NAME --host HOST` address a catalog product's one unit on HOST before the registry records it: the unit `service ensure` has just created, and whose refusal to stay up is the thing to read, answered `NAME is not a registry-managed service on HOST`.
- The host channel reads its fleet SSH key as named; native signing reads the Apple certificate item as named; a `product` operation runs on a thread sized for the dependency walk (a debug build overflowed the 2 MiB a tokio blocking thread offers).
