# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.66 (part 1 of 2)](changelog/0.23.66-part-1.md)
- [0.23.66 (part 2 of 2)](changelog/0.23.66-part-2.md)
- [0.23.42 – 0.23.65](changelog/0.23.42-0.23.65.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

### Fixed

- **A new build sheds the run it keeps:** a build run keeps the previous attempt to measure the next one's free space, and a run an older Stado left whole kept its source export and build output until two more builds replaced it. The previous attempt now keeps only its files and the size it recorded; the free-space check reads that size.

- **Azure and Apple signing credentials are read by role:** the billing collector's Azure section reads the service principal of the item tagged `stado:role:azure-billing`, the Azure token chain reads `cloud-azure` like every other cloud provider, and native signing reads the certificate of the `macos-development-signing` role. `WC_AZURE_BILLING_SECRET` and `WC_AZURE_SECRET` are gone: the billing setting named an item id that was then looked up as a role, so a configured principal reported `no_credentials`. Tag the billing principal with `stado credentials item retag --host <vault owner> <item> --tags stado:role:azure-billing`.

### Added

- **`stado web route` publishes a `cloudflare`-edge hostname through the Cloudflare tunnel:** it refused every such hostname and named `stado tunnel route` with item names to type, so a host behind a residential uplink, whose 80 and 443 nothing outside can reach, published nothing. The route now resolves the connector host and origin (a product's own host and loopback port, or the active host and endpoint of the service it fronts), reads the items playing `stado:role:cloudflare-api` and `stado:role:cloudflare-tunnel` in the owner vault, and runs the same ingress, connector token and proxied `CNAME` steps `stado tunnel route` runs; `--check` prints that plan and changes nothing. Refusals: a role no item plays (`no item in the owner vault on <owner> plays role cloudflare-api; tag the item … with stado credentials item retag …`), a mount or a redirect on the cloudflare edge (a tunnel route carries a whole hostname to one origin), a fronted service the directory does not declare or that has no endpoint on its active host, and a connector service not declared on that host. The zone must be served by Cloudflare first (`stado dns delegate <zone> --provider cloudflare`). `stado web remove` still leaves the tunnel route and names `stado tunnel remove`.
