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

- **`stado credentials put --role ROLE` writes the item that plays a role:** the payload on standard input replaces the one live item tagged `stado:role:ROLE`, so a writer, like a reader, names no item. When no item plays the role yet, it is created under a fresh random id and tagged `stado:role:ROLE`; several items in the role are refused. `--role` is not accepted with `--route`: a consumer's rotate grant names an item.

- **`stado azure login --role ROLE` and `stado azure repair-rbac --operator-role ROLE`** (default `stado-azure-operator`) replace `--item` and `--operator-item`. Login stores the operator session in the item that plays the role, and repair-rbac reads it back by that role; before, login wrote an untagged item the role read could never find.

- **A catalog service declares its port once:** `service.listen_port` is the one TCP port a catalog service listens on, its arguments and environment name it as `$STADO_LISTEN_PORT`, and catalog validation refuses two services that declare the same port, since one host runs every catalog service. Skrzynka and Weles both declared 8788; Skrzynka now listens on 8792. `stado weles activity` reads Weles's `listen_port` instead of a built-in 8788.

- **A scoped consumer reads a secret by role:** `stado credentials get --role ROLE --field F --route … --consumer … --grant-file …` asks Skarbiec for the coordinate `role:ROLE` instead of listing the vault, which a consumer granted only its own fields cannot do. The consumer's grant is `read:role:ROLE#F` (Skarbiec 8bde5b5 or newer), and Skarbiec reads the one live item tagged `stado:role:ROLE`; no holder, or two, answers as an absent field. Without `--route` Stado still finds the item by listing, as the store administrator.

- **A credential read that fails states why:** every vault failure carried into a command states the class Skarbiec's answer decides (`not_found` for an absent item or field, `auth` for a refused identity, `refused` for a refused grant, `infra_down` for an unreachable or unavailable vault, `config` for a client that is not configured), and a read that found the item without the field it needs states `not_found`. This covers `stado credentials get|put|rotate|ls|rm`, the Azure, Cloudflare, registrar, Supabase and fleet-database reads, the release signing key and publisher token, the host-health beacon, the verifier shadow and service secret delivery, all of which printed `the command failed and we could not attribute the failure` (5b3bd385, in part).

- **A release, build or report that cannot open the fleet store states why:** every `stado release`, `stado build`, report and host-service path that opens the fleet store now states the store's own class (`not_found`, `auth`, `refused`, the upstream status for an object API or GCS answer, `infra_down` for an unreachable store) instead of `the command failed and we could not attribute the failure` (5b3bd385, in part).

- **A host or configuration that cannot be used states why:** every `stado host …`, `stado secrets …`, `stado setup`, `stado space report`, release-enrol and route command that names a host now refuses with `not_found` when the registry does not hold that host, `refused` when the host is registered but not reachable through the host channel, and `infra_down` when no registry answered. A missing or unreadable `~/.config/stado/config.json` states `config`, and an unreadable capacity publication or JSON answer states its own class (5b3bd385, in part).

- **A busy disk-cleanup pass reports the declared watermark:** a pass that finds the cleanup lock held keeps the previous pass's reclaim state but now reports the low and target watermarks the registry declares for the host, and its pressure against them. It used to carry the previous pass's watermark, so `stado host disk-cleanup` could publish 2 GiB while the registry and `stado space report` said 8 GiB, and build placement, which reads the published watermark, sent a release build to the vault owner with 2.8 GiB free (8221e76d).
