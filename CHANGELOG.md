# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.0](changelog/0.22.18-0.23.0.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- A release pass no longer records its failure over a run another pass has advanced since it read it. The control host's release agent and `stado release resume` both walk a run; the pass that lost the write race (`Stado storage version changed for runs/release-pipeline/<run>/run.json`) marked the run `failed`, and every delivery worker then refused its job with "the run is Failed, not delivering".
- A release pass over a run whose platforms are all published keeps it `delivering` instead of walking it back through `waiting` and `publishing`, and a delivery worker accepts a live run (`waiting`, `publishing` or `delivering`) whose platform and digests match. Before, every delivery that started while the release agent's tick or `release resume` was mid-walk refused its job with "the run is Publishing, not delivering".
- `stado service stop`, `retire` and `remove` judge a systemd unit by what `systemctl is-active` answers, and an unmet check names the scope, the unit file and that answer. The exit status of `is-active --quiet` read as active for units systemd itself called `inactive`, so stopping or retiring a stopped unit failed and its registry declaration was restored.
- A host's `account_ref` credential is read as the item the registry names. Selected by role, it answered nothing for host-account items, which carry the host's tags and no `stado:role:`, so every stop, retire or release of a system LaunchDaemon refused with "has no readable host-account password" while the item held one.
- Database creation validates and normalizes its declaration before fleet placement, Supabase requests or external credential writes. Invalid database names and consumer identities report the same usage refusal as `declare`, rather than reaching a dependency or provisioning a resource before rejecting its consumers. The prepared declaration is persisted only after provider success; configuration write failures remain errors and are not a rollback guarantee.
