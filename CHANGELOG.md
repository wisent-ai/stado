# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.62](changelog/0.23.42-0.23.62.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **Azure tokens are replaced by measured margin, not five minutes:** a cached ARM or Blob bearer was renewed 300 s before its expiry, and a token answer without `expires_in` was read as one hour (fab304dc). A cached token is now fresh while one more acquisition, as long as the last one took, would still end before it expires; an IMDS or client-credentials answer that states no `expires_in` is refused with `IMDS response states no expires_in` or `<item> client-credentials response states no expires_in`.

- **CPU capacity is measured over whole clock ticks, not a 250 ms window:** two readings inside 250 ms shared one answer. The kernel's processor counters move in whole ticks, so a reading taken before one has passed since the baseline keeps the last answer and the baseline; any later reading measures the busy share over everything since it (fab304dc).
