# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.22](changelog/0.23.12-0.23.22.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A service release reaches the unit the service directory names for it (ae478d1f):** `release_control.products.weles-worker.service` is the directory service `weles-admission`, and `service_directory.services.weles-admission.managed_service` is `com.wisent.weles`, yet the gate compared the release only with the unit's own names and a legacy label, so every Weles release ended `product "weles-worker" releases service "weles-admission", not unit "com.wisent.weles"`. The gate now also accepts the unit the directory links to that service (its `managed_service`, or the unit its placement profile installs), and a refusal says that neither the directory nor the release target links them.
