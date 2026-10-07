# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.56](changelog/0.23.42-0.23.56.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **The vault owner's channel opens without that vault:** every host channel command (`stado repair`, `stado host unit-log`, `stado service logs`, deliveries) asks Skarbiec for the host's `stado-ssh-<host>` key and used the key that vault last handed out (`~/.stado/host-keys/<host>`) only when Skarbiec answered with an error. On charless-mac-mini Skarbiec's gpg waited on a held key database and never answered, so every channel command for the mac waited too — `stado repair skarbiec --step crypto --target charless-mac-mini --apply`, the declared repair for exactly that, included — and the fleet's object API stayed `503 object authorization unavailable` for almost an hour (04886338, 3f9201e7). For the host the service directory names as Skarbiec's, the held key is now used before the vault is asked; every other host still asks the vault first.
