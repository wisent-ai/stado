# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.23](changelog/0.23.12-0.23.23.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **The worker survives a fleet store that does not answer (3c4bb46a):** a 502 from the vault host's object API, or a connection it refused, ended `stado serve` through its worker role (`agent loop failed: Stado object API error HTTP 502`), and the host's resolver and every service forward went down with it until launchd restarted the process, so every local client of those forwards, Tama's hooks among them, timed out. A tick whose fleet store answers 5xx or cannot be reached now prints `tick did not run: the fleet store did not answer (<cause>); the next poll runs again` and the worker keeps polling; any other error still ends it visibly.
- **A handoff refuses a commit its product's own gates refuse (d49e7f65):** `stado release changes submit` runs the lock and formatting gates `stado quality check` runs over the handed-off commit before any ticket exists, and refuses `<commit> is not handed off: stado quality check: gate "fmt" of <product> refuses …`. An unformatted commit no longer reaches the batch build and fails every other session's work in it; the gate's own output goes to stderr, so `--json` keeps one ticket on stdout.
- **Build secrets enrolled on a running builder are offered (42722064):** an agent's capacity publication sent the `agent.skarbiec.secret_fields` it loaded at startup, so a coordinator refused a builder (`github#value is not in its agent.skarbiec.secret_fields`) whose config already held the field. The publication now joins that list with the config file as it is now, the reading the agent's own admission already used.
