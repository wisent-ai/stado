# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.22.19](changelog/0.22.18-0.22.19.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- The catalog's `com.wisent.stado` declares `--health-interval-seconds 60`, so every host's one process publishes the host's health beacon; before, the role was only folded in from a predecessor beacon unit, and a host that had none published nothing, which `stado service list` reported as `unknown` for every unit on it. A process that serves the host's API (`--api-local-store`) writes the beacon straight into the store it serves, with no network hop and no beacon grant; any other process still publishes through `STADO_HOST_HEALTH_API_URL` with the grant its environment names.
- A build worker reads only the product and its platform's recipe from `.wisent-release.json` (`parse_worker_manifest`); deliveries, promotion, inputs and runtime are kept as values it does not parse. A builder runs the Stado its host already has, so a section that changes shape in a commit — a delivery `target` that became the product's destination declaration — no longer stops the build that carries its reader on every builder still on the previous release. The manifest's deliveries name their hosts again until every builder reads the destination form; the control host resolves either.
