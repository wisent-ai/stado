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
- `stado host user create` takes `--json` and prints `{username, dry_run, hosts: [{target, ssh_target, status, os, detail}]}`; any failed host still exits 1. The initial-password prompts now go to standard error, so standard output carries only the answer in both forms.
- `stado vast` is replaced by `stado market`, with the marketplace named by a required `--provider` (`vast` is the one adapter): `stado market list|unlist|status|readiness|monitor|auto-list --provider vast`. A provider is an adapter behind the product's operation, not the operation (cli.md rule 14). No price, idle window or rental cap has a built-in value any more: `list` needs `--price-gpu` and `--price-disk`, `auto-list` needs `--idle-window-s`, `--price-gpu` and `--max-duration-s`, and `monitor --bucket` falls back to the configured queue bucket instead of a fixed name. The MCP tool `stado_vast_status` is `stado_market_status`, the Desktop operator API accepts the `market` family, and Stado Desktop's Earning screen runs `stado market --provider vast` with a GB-month price field beside the GPU-hour one. Readiness remedies, the auto-list refusal and the credential refusal name `stado market readiness --provider vast` and the item `vast/api_key` that is actually read. `examples/providers/enable-vast.sh` writes the key into the `vast` item through standard input instead of an argument.
- `stado dns list|set|remove|delegate|undelegate` no longer fall back to the built-in vault items `namecheap_auto` and `cloudflare-api`: `--credential` (the registrar's item) and, for `delegate` and `undelegate`, `--api-credential` (the Cloudflare item) are required, and a missing one is refused by the parser with exit 2 before anything is read (cli.md rule 14).
- `stado web route` and `stado web remove` no longer write a hostname's record through the built-in vault item `namecheap_auto`. The edge declaration names the registrar's item: `stado web edge declare --target <host> --address <ipv4> --contact <mail> --registrar-credential <item>` writes `web_api.edge.registrar_credential`, `stado web edge provision` and a later `declare` without the flag keep the one already declared, and `--json` reports it. A route or removal on an edge that declares none is refused with `web_api.edge declares no registrar_credential, so no Skarbiec item can write this hostname's record; …` before the registrar is called (cli.md rule 14). The refusals that point at `stado dns list` name the declared item.
- `stado database create --provider supabase` without `--anchor` no longer joins the project of a built-in database named `oko`. It takes the one organization and region every project the Supabase token sees shares; a token that sees several is refused with each `<organization> (<region>)` named and `--anchor` asked for, and a token that sees none is pointed at `--provider fleet` or `--anchor` (cli.md rule 14).
- Three commands no longer assume one product, provider or account (cli.md rule 14): `stado blast-radius` needs `--dependency` (it assessed `gcp` when none was named), `stado host secrets apple` needs `--credentials` (it read the `wisent-apple-notary` item), and `stado release activate-staged` needs `--product`, `--env-file` and `--port` (it activated `weles-worker` from `$HOME/.config/weles/worker.env` on 8788). A missing one is refused by the parser with exit 2.
- `stado submit` and `stado schedule add` no longer run `pip install '.[train]'` on every cloned repository. `--repo-extras` is empty unless named, so a repository without a `train` extra is cloned and run as it is; the machine request API and profiles use the same empty default (cli.md rule 14). Records and schedules written before keep the extras they carry.
