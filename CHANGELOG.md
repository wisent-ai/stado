# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.16](changelog/0.23.12-0.23.16.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A product's installation can declare what it needs in Stado, and Brama's service installation declares its maintenance schedule:** an `after_install` step may run `stado` itself besides the installation's own binaries, and `{install_id:NAME}` is a UUID derived from the product, surface, host and NAME, so a step that creates lasting state under it is the same declaration on every reinstall. Brama's service recipe now runs `stado schedule create --id {install_id:maintain} --pinned-host {host} --cron '* * * * *' 'brama maintain --gateway-consumer brama-desktop --bearer-role brama-console'`, and the service depends on Brama's CLI on that host. Nothing ran `brama maintain` on the gateway host after its renewal unit was retired, so no subscription grant was renewed (1cff0c87). The pass reads the console bearer from the item tagged `stado:role:brama-console`, which the vault owner tags once its Skarbiec carries the `stado:role` namespace.
