# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.3](changelog/0.22.18-0.23.3.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado release catalog enroll <product>` rolls a product whose catalog service names its one unit (`com.wisent.<product>`) out by `replace`: a new rollout policy is created with `replace`, and a `blue-green` policy the product already has is converted (`rollout policy converted from blue-green to replace of its one unit <unit>`), its targets losing the stable bind and candidate ports. Before, every product got a blue-green policy, which runs it as release processes behind a proxy in the host's Stado beside its own unit.
- The release agent hands a product's port to its unit only when its release proxy holds exactly the port the service directory names for that unit; otherwise it stops nothing and says which port the proxy does not hold. The unit is the route's `managed_service`, or, for a placement-backed route (Brama's), the unit its placement profile names for that host.
- `stado dns delegate` and `stado dns undelegate` take the DNS host as a required `--provider` (`cloudflare` is the one host with an adapter; any other value is refused with the list of hosts). The verbs no longer name one provider in their help, so a second DNS host is a new `--provider` value, not a new command.
