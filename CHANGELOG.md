# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.61](changelog/0.23.42-0.23.61.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A release delivery reaches a host whose janitor never stops:** a host that cannot get under its disk watermark runs janitor passes back to back, each holding the workload lock for its whole length (about twelve minutes on lukasz-macbook), and the signed Stado release delivery — the one job admitted under disk pressure — found the lock taken at every claim. Stado 0.23.61 stayed `delivering` with that delivery `queued … not yet claimed`, and Oko refused to close any defect it carries (a7c056fd). That delivery now starts beside the pass, logged as `<job>: a janitor pass holds the workload lock; the signed Stado release delivery starts beside it`; every other job still waits for the lock. No cleaner takes what a running delivery uses: its work tree belongs to a job the queue does not report terminal, and `delivered_releases` keeps each product's newest version.

- **`stado release catalog pin-input --swiftpm` publishes a Swift package's resolution:** every SwiftPM desktop release unpacks a `swiftpm-cache` input into its source and builds offline, and every one pinned an archive made by hand in August that the fleet store no longer holds; `stado quality check` refused most-desktop, oko-desktop and brama-desktop with `… answers is absent`, and no verb could make another (b6996b35). `--swiftpm` runs `swift package resolve --disable-automatic-resolution` into a scratch of its own, packs the resulting `.build/` (checkouts, repositories, artifacts, workspace state) in name order with cleared owners and times, stores it create-only under `sources/<product>/dependencies/<name>/sha256/<digest>/source.tar.gz` and pins it with mount `<name>.tar.gz`, `extract: false`. A `Package.swift` or `Package.resolved` that differs from the revision is refused. Stado Desktop's **Publish an immutable build input** has the same toggle. Used for most-desktop: 1,611,520,610 bytes, sha256 `4e3bbaca…`, and its quality check passes.
