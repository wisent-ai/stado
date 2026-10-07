# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.39](changelog/0.23.12-0.23.39.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A pending change whose commit its repository lost no longer stops the product's releases (75e10247):** `stado release submit` and `stado build submit` bind every pending change of the product whose commit the release contains; a change recorded against a commit that origin's rewritten history no longer carries made `git merge-base` fail and the whole submit end `cannot prove release coverage: fatal: Not a valid commit name …`. Such a change is now left out of the release and named on stderr with its id, task and commit, so the work it stood for is recorded again on a commit main carries.

- **`stado_database::connect` opens Postgres on the item's `session_url` (35a1f379):** a Supabase item's `pooler_url` is the transaction-mode pooler, which hands each transaction another server connection, and SeaORM through sqlx names every statement (`sqlx_s_1`, …), so every product query failed `prepared statement "sqlx_s_1" already exists`. `stado database create` and `adopt` now write `session_url` (the same pooler on its session port, from Supabase's Supavisor documentation), `place` and `--provider external` write it as the server's own URL, and `connect` reads it for Postgres; the synchronous client's unnamed-statement path is gone. New `stado database client NAME` widens `<NAME>-database-client`'s grant on the vault owner to the fields the crate reads (`pooler_url`, `ca_certificate`, `session_url`), keeping its bearer; Stado Desktop's row menu runs it as **Grant library client reads**. An item without `session_url` is refused at `read credential field` naming `adopt` or `create` as the repair.

- `stado service directory consumer-add --target HOST` no longer takes `--bind LOOPBACK:PORT`. The host hands out the loopback port its resolver adapter listens on (`stado host free-port-local`, the same source a catalog service's port has), the registry records it, and the command prints the address it recorded. A consumer the host already routes keeps its address, so declaring it again never moves a port under a running client. Stado Desktop's form drops the port field. An adapter that still carries a port a person chose gets one from its host with `consumer-rm`, then `consumer-add --target HOST` with the consumer's capabilities.
