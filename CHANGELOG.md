# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.4](changelog/0.22.18-0.23.4.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado service ensure` checks that the unit's program is on the host before it retires any unit the product replaces, and refuses with `<host>: <product> runs <program>, which is not on the host; install it first (…). Nothing was retired or started` (`error_code=refused`). Before, it unloaded the replaced units, failed with `program_missing`, and loaded them again.
- A `database_api` section (or `WC_DATABASE_API_DATABASES`) that does not parse is reported as a configuration failure (`error_code=config`) by `stado database list`, `resolve`, `destroy`, `supabase adopt`, `stado web declare --database` and `stado web deploy`. It was printed with the right sentence but reported as an unattributed failure of Stado (`error_code=unknown`).
- `stado credentials get`, `put`, `ls` and `rm` state what kind of failure a vault answer is: a stored ciphertext envelope or a missing grant file is `config`, a refused grant `auth`, an absent item or field `not_found`, another refusal by the vault `refused`, an unreachable vault `infra_down` (retryable). Every one of them was reported as an unattributed failure (`error_code=unknown`). The class comes from the failure's type and the vault's HTTP status, not from its sentence.
- A registry write that proceeds past a fault already in the registry names the section that holds it (`` `release_unit_image_revisit`: … `` or `` `inference`: … ``). It always said the `inference` section was invalid, also when the fault was in the unit-image revisit policy.
