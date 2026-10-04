# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.17](changelog/0.23.12-0.23.17.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A scoped consumer reads a secret by role:** `stado credentials get --role ROLE --field F --route … --consumer … --grant-file …` asks Skarbiec for the coordinate `role:ROLE` instead of listing the vault, which a consumer granted only its own fields cannot do. The consumer's grant is `read:role:ROLE#F` (Skarbiec 8bde5b5 or newer), and Skarbiec reads the one live item tagged `stado:role:ROLE`; no holder, or two, answers as an absent field. Without `--route` Stado still finds the item by listing, as the store administrator.

- **A credential read that fails states why:** every vault failure carried into a command states the class Skarbiec's answer decides (`not_found` for an absent item or field, `auth` for a refused identity, `refused` for a refused grant, `infra_down` for an unreachable or unavailable vault, `config` for a client that is not configured), and a read that found the item without the field it needs states `not_found`. This covers `stado credentials get|put|rotate|ls|rm`, the Azure, Cloudflare, registrar, Supabase and fleet-database reads, the release signing key and publisher token, the host-health beacon, the verifier shadow and service secret delivery, all of which printed `the command failed and we could not attribute the failure` (5b3bd385, in part).

- **A release, build or report that cannot open the fleet store states why:** every `stado release`, `stado build`, report and host-service path that opens the fleet store now states the store's own class (`not_found`, `auth`, `refused`, the upstream status for an object API or GCS answer, `infra_down` for an unreachable store) instead of `the command failed and we could not attribute the failure` (5b3bd385, in part).
