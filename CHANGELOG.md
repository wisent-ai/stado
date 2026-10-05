# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.34](changelog/0.23.12-0.23.34.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **Main builds under the release's clippy gate again (5ad96421):** the uncalled `cancel_in_store` is gone, the Azure VM generation check is a match guard, and the shared `tests/desktop_api` fixture's two per-journey constructors are marked as shared, so release builds no longer stop at `-D warnings`.
- **A replaced unit image is mid-flight while its installer lives, not for five minutes (fab304dc):** `IMAGE_SETTLE_SECONDS` (300) is gone. `release install-local`, `release converge-local-readers` and self-update write `.<name>.replacing` beside each file they replace, holding their pid, from the first byte written until every unit on it is recycled, and remove it when they finish. `registry doctor` and `service refresh-image` read a unit on the old image as mid-flight while that pid is alive and as `stale-unit-image` once it is gone, however recently the file was written; `refresh-image` names the installer (`installer pid N is replacing that file right now …`).
