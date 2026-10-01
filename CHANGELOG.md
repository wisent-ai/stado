# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per range, because a file this repository cannot edit is
a file that stops receiving entries: the length gate refuses every write to a
file past 300 lines, and this one had reached 414. Two product fixes on
2026-09-08 could not be recorded at all until it was split.

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

- `stado schedule edit ID [--command C] [--cron EXPR] [--tz ZONE] [--json]` changes what a schedule submits or when, through the same compare-and-swap update pause and resume use; an enabled schedule's next run is recomputed from now unless an occurrence is already leased. Naming none of the three is a usage error, and an invalid cron or a timezone the next run cannot be computed in is refused before anything is written.
- `stado artifact alias remove ALIAS_REF --expected-target VERSION [--json]` deletes an alias while it still targets VERSION; an alias retargeted since is refused with `ARTIFACT_ALIAS_CONFLICT` and nothing is removed, the versions stay, and an absent alias answers `removed: false`.
- `stado host gpu-power-limit-unset TARGET [--json]` withdraws a declared board power cap: it removes `gpu_power_limit_watts` from the registry target through the same compare-and-swap write `gpu-power-limit` uses, then sets every GPU back to the driver's `power.default_limit` and prints what the driver reports. A failed host step names the registry generation already written and the host's own error.
- `stado space volume unmount TARGET --mount-point PATH [--json]` is the inverse of `volume mount`: it unmounts the filesystem and removes the `# stado-volume` fstab line for that mount point, leaving the disk's data alone. A busy filesystem is refused with `umount`'s own error and fstab is left unchanged; a mount point declared by an fstab line stado did not write is refused by name; a mount point neither mounted nor declared answers `not_mounted` and changes nothing.
- `stado fleet unassign TARGET` takes a registered machine out of its fleet by clearing the target's `fleet` field through the registry's compare-and-swap write; the machine stays registered, an unknown target is refused, and a target in no fleet is reported unchanged. `fleet delete`'s refusal for a fleet with members now names both `fleet assign` and `fleet unassign`.
