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

- **The agent re-asserts its host's declarations every tick, and the spawn watch takes the length and gap it is given (fab304dc):** the 300-second GPU power-limit and placement-policy reconcile intervals are gone; both are re-asserted at the agent's declared poll, so a drifted power cap or policy file is corrected within one poll. `stado service watch-spawn` no longer refuses a `--seconds` above 3600 or an `--interval-ms` outside 200–10000; it refuses only 0 (`watch length must be a positive number of seconds; 0 watches nothing`, `sample interval must be a positive number of milliseconds; 0 samples nothing`).
- **Coverage verification runs as wide as the machine and lets the server set the pace (fab304dc):** `COVERAGE_VERIFY_THREADS` (4, chosen to stay under a remembered Hugging Face limit) and `COVERAGE_PROGRESS_LOG_EVERY` (200) are gone. `stado coverage` verifies and scans failed commands at the operating system's parallelism; a HEAD answered 429 with a `Retry-After` (seconds or an HTTP date) is waited out and asked again, and a 429 that states no wait still fails with its status. The walk logs one `[<universe>] N/N verified` line when it has classified every entry.
- **No failure-fixer cadence or selector is written into Stado (fab304dc):** `FAILURE_FIXER_TICK_SECONDS` (180) and `FAILURE_FIXER_COMMAND_PATTERN` are gone. They only filled the argv of a legacy failure-fixer unit plan that is never installed; a captured unit is folded into the host process with its own `sleep` as `--failure-fixer-interval-seconds` and its own `--command-pattern`, and the plan now names only the program it is compared against.
- **The claim walk lists what it can still use, and every queue read fans out as wide as the machine (fab304dc):** the 256-name `MARKER_PAGE` is gone; each page of the priority index asks for as many names as the scan still wants or may still download, whichever is fewer, and an unbounded scan takes the backend's own pages. The remaining fixed fan-outs — 10 marker and job reads per page and per listing, 32 in the oldest-first scan, makespan's agent reads and sizing's history reads, 16 makespan writes, 8 release-run report reads — now use the parallelism the operating system reports, like the bulk copies. The activation-manifest refusal names every benchmark missing target metadata instead of the first ten.
- **A shut API boundary is revalidated one sweep at a time, not once per 30 seconds (fab304dc):** the 30-second recheck cooldown and its `WC_DASHBOARD_BOUNDARY_RECHECK_SECONDS` override are gone. A request that finds a boundary closed revalidates it inline unless another request's revalidation is already running, in which case it is refused at once; a request whose client goes away mid-sweep frees the claim, so a closed boundary is retried by the very next request after a failed sweep instead of half a minute later.
