# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.58](changelog/0.23.42-0.23.58.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`serve --api` starts without declared request limits:** since c9ee2a35 the API listener refused to start unless `dashboard.request_limits` (or `WC_DASHBOARD_REQUEST_LIMITS`) declared `head_bytes`, `body_bytes`, `registry_import_bytes`, `operator_console` and `fleet_join`; no host declared them, so stado 0.23.59 on charless-mac-mini crash-looped with `API listener preparation failed: API request limits are not declared` and every Stado command in the fleet answered 502 until 0.23.56 was restored there (d6b3c3ce). Nobody stated those values and the operator is not asked for numbers, so a deployment that declares none now bounds every request by the memory the host can give it at start (`read_host_memory`'s available bytes) — the source store document reads already use — and the argument count by the same figure. A declaration still sets them; a host whose memory cannot be read refuses to start, naming both keys and the reading.

- **The janitor takes delivered release copies:** every delivery stages the release it installs under `~/.stado/releases/<product>/<version>/<platform>/`, and no cleaner covered that tree, so a host kept every version it was ever delivered; the Linux builder carried 7.9 GB of them under `/root/.stado/releases`, sat above the 80 % disk-full threshold with every cleaner reporting nothing eligible, and refused release builds as `cleanup_in_progress` (f036eefe). The new `delivered_releases` cleaner takes every version directory except, per product, the one `~/.stado/bin/<product>.release-version` names, the one whose staged copy is byte for byte the installed binary (what `stado service converge` attests against) and the newest by modification time (`installed_or_newest`). On lukasz-macbook a dry run counted 60 versions, 51 eligible, 6.07 GB.
