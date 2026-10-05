# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.35](changelog/0.23.12-0.23.35.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A shut API boundary is revalidated one sweep at a time, not once per 30 seconds (fab304dc):** the 30-second recheck cooldown and its `WC_DASHBOARD_BOUNDARY_RECHECK_SECONDS` override are gone. A request that finds a boundary closed revalidates it inline unless another request's revalidation is already running, in which case it is refused at once; a request whose client goes away mid-sweep frees the claim, so a closed boundary is retried by the very next request after a failed sweep instead of half a minute later.
