# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.5](changelog/0.22.18-0.23.5.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado product install|update <product> --surface service --host <host>` refuses a host other than the machine it runs on before anything is built (`a service installation puts its files on the machine that runs this command, so --host <host> would restart <host>'s unit on the files it already has; run the installation on <host>. Nothing was built, installed or restarted`). It built and installed the service's files on the local machine and then restarted the unit on the named host on its old files.
