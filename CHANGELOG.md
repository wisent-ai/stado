# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.2](changelog/0.22.18-0.23.2.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado credentials put NAME --field F --route URL --consumer C --grant-file FILE` replaces one field of an existing item under that consumer's own `rotate:NAME#F` grant, through Skarbiec's `PUT /v1/items` rotate mode: the other fields, kind, recipients and tags stay and the consumer is recorded as the writer; the value comes from stdin. In 0.23.2 `put` took `--route`, `--consumer` and `--grant-file` but still wrote the whole item to the owner vault as the store administrator. `--type` cannot be combined with `--route`, and a delegated `stado credentials get` again requires `--field` (a whole-item delegated read looked the name up as a role).
- `stado service ensure` registers a product's acquisition scopes before it starts the product's unit, when the catalog entry names the scope catalog the installed release carries (`acquisition_scopes`; Weles does). Before, Weles placed on a host whose vault had never seen its scopes crash-looped on Skarbiec 401s until someone ran `stado credentials acquisition-scopes sync` by hand. A release without the file, or a vault that refuses, fails the ensure before anything is retired or started.
- `stado web schedule set|remove|list` declares the requests a web product's unit is sent on a cron (path, method, cron, IANA time zone, and optionally the header and `role#field` secret that prove the caller), under `web_api.products.<name>.schedules`. `stado web route` creates one fleet schedule per entry, pinned to the product's host, only after the hostname answers from this fleet, so a product still served by another platform is never called twice; it keeps, replaces or deletes them to match the declaration, and `stado web remove` deletes them. `stado web declare` keeps a product's schedules when it rewrites the declaration.
