# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.24](changelog/0.23.12-0.23.24.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **Acquisition-scope registration keeps its grants' lifetime instead of relying on Skarbiec's removed default (1f9125eb):** Skarbiec 0.4.8 refuses `token-register-acquisitions` without `--ttl-seconds`, so every `stado credentials acquisition-scopes sync` and `stado service ensure weles` failed with `token-register-acquisitions requires --ttl-seconds`. Registration now passes the remaining lifetime of the catalog's current registration (the earliest unexpired grant among its consumers), so re-registering never extends it; `acquisition-scopes sync --ttl-seconds <seconds>` states a lifetime, and a catalog with no current registration and none stated is refused with `no grant of this catalog is current in <vault>, so there is no lifetime on record to keep, and Skarbiec requires one: state it with stado credentials acquisition-scopes sync --host <host> <catalog> --ttl-seconds <seconds>`.
- **Registering an acquisition catalog settles the roles its rows name (648fa7af):** `stado credentials acquisition-scopes sync` and `stado service ensure` registered `role:<role>` rows whether or not any vault item played the role, so after Weles's catalog moved from item names to roles every read failed at startup with `acquisition field does not exist on item`. Registration now reads the host vault for each role: a role nobody plays whose former item (the live item of the same id, playing no role) exists is given `stado:role:<role>`, its other tags kept. The answer carries `roles: {held, adopted, unadopted, unheld, contested}`, and each role that still cannot be read is named on stderr with what to do: `stado credentials item put --host <host> --role <role>` for one nobody holds, the vault's own refusal for a former item it would not retag, and the tag count for one several items play.

- **`credentials item upgrade` says how many items a former owner still controlled (57a7f19d):** Skarbiec 0.4.8's `upgrade` moves items a former vault owner still controls to the current owner, because nothing could write them after `rotate-owner`. The text report prints `<host>: items a former owner controlled: <n> would move to the owner` (`moved to the owner` with `--apply`), and `-` from a Skarbiec build without the step; `--json` carries it as `pass.control`.
