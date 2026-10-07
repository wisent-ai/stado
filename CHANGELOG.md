# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42](changelog/0.23.42.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A replace delivery waits while the running service names work a restart would end:** before restarting a unit, `stado service release` and the pipeline's replace step read the service's readiness answer once; a non-empty `in_flight` list (Weles names its running runs there) leaves the unit untouched. The release run stays delivering with `failure: delivery waits: <target> runs <service> with work a restart would end (run_id=… action=…)`, and the control host's next release-agent tick reads the service again. An operator's `stado service release` is refused with the same list. A service that names no `in_flight`, or does not answer, is restarted as before. Weles releases used to land while a sign-in waited on the operator's phone and killed the run mid-wait.
