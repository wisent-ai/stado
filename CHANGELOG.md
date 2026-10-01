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

- `stado fleet ingress down` and `stado fleet key add|install|check|generate|rotate` take `--json`. `ingress down --json` prints `{published, stopped, base_url, tunnel, listener, unpublished}`; the key commands print the target with what each did: the stored `item` and `fingerprint` (`generate` adds the `public_key`), the `destination` installed into or answered from, and for `rotate` the `from` and `to` fingerprints.
- `stado fleet enroll` and `stado fleet approve` take `--json`. Progress — the registration, the key adoption, the bootstrap's own lines — goes to standard error, and standard output carries one document: `enroll` prints `{target, hostname, kind, fleet, generation, bootstrapped, offline_invite_spent}`, `approve` prints `{approved, target, generation, fleet, enrollment, invite_spent, install_with}`. The key adoption inside `enroll --install-key` now narrates on standard error in both forms. `approve` of a request without a channel registers the machine and places it in `--fleet` in one registry write, where it took two.
- The lease reaper reaps every expired lease even when one running job cannot be read. A record it cannot derive — `run id … has a planned job not derivable from its request` — is logged as `<job>: not reaped: <error>` and counted as `could not read N`, where it ended the whole pass and left every dead job behind it holding its slot and the cleanup lock.
- `stado service env-set` on a systemd unit no longer puts the value in the host's process list. The host primitive `service unit-env-local` takes `--value-stdin` and reads the base64 value from standard input, piped by the shell's `printf` builtin; `--value-b64 <value>` is gone. A host whose installed Stado predates this refuses `--value-stdin` as unknown; `stado bootstrap` brings it up to date.
