# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.19](changelog/0.23.12-0.23.19.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A release published on the host that serves the store reaches every host (282f15d6):** on that host every Stado process except the object API is a queue client rooted in `ecosystem/probierz/`, and `stado storage put`, `get`, `stat`, `objects`, `rm`, `abort-upload` and the release publisher resolved `stado://<namespace>/<key>` under that root. The release agent therefore wrote stado 0.23.19 to `ecosystem/probierz/ecosystem/releases/stado/0.23.19/…`, read it back there, marked both platforms published, and every delivery failed with `input archive is absent: stado://releases/stado/0.23.19/<platform>/release.tar.gz`. A `stado://` object is now resolved from the top of the store, where the object API serves it; a bare queue path given to `stat`, `cat` or `ls` keeps the queue client's root.
