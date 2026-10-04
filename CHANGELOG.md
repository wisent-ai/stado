# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.20](changelog/0.23.12-0.23.20.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A release whose build input belongs to a declared publisher no longer re-declares it by role (a002375c):** enrollment checked an input's publisher by its role while the build reads `release_api.publishers.<product>.item` as named, so on a vault whose Skarbiec predates role tags the check found nothing, Stado declared the publisher again, the vault refused `tag stado:role:skarbiec claims a namespace that is not registered`, and Skarbiec 0.4.7 — the release that registers role tags — could not be submitted. Enrollment now reads the declared item exactly as the build does.
- **`stado product install --surface service --release-version V --source-commit C` installs a stado-release service from its exact, verified release (ad7c7ea8):** the coordinate was accepted only for a CLI on the local machine, so a service outside release control could be installed only by a source build on its host. The service is installed on the host the command runs on, from the archive that version's signed manifest names for that host's platform, and its unit ensured as before. A CLI given `--host` with a release coordinate is refused with the reason.
- `stado product catalog --names [ORG]` writes the workspace name register
  (`~/NAMES.md`): every active repository of the organization (default
  `wisent-ai`) as `gh repo list` reports it, with its GitHub description and
  the catalog product that claims it. `--output PATH` writes it, `--check
  PATH` refuses a register that differs. It replaces the `scripts/gen-names.sh`
  the register named but nothing held, so a deleted repository leaves the
  register when it is regenerated.
