# Changelog Unreleased (part 10)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **`stado cloud host create|list|delete|reap --provider azure` rents fleet hosts from the cloud
  under the operator's credit floor:** `create NAME --location L --size S --disk-gb N
  --idle-minutes M` prints the projected hourly burn (the new machine's on-demand hour and that
  of every cloud host already running) and holds the create to the Azure credit gate with all of
  them counted, measures the public address of each host `cloud_hosts.admit_from` names (each asks
  the product site's `/api/egress` over its host channel) and admits only those to SSH, mints the
  host's fleet key as `fleet key generate` does, creates the public address, security group,
  interface and VM (cloud-init installs the build toolchain and keeps sshd key-only), enrolls it
  through the verified `fleet enroll` path, records `targets[].cloud`, waits for the first boot
  to end and reports the toolchain it answers. A host that cannot be enrolled is deleted again.
  `reap [--apply]` readmits the fleet's current addresses and deletes each host whose run area has
  stood empty longer than its idle minutes; the coordinator runs the applied pass on every tick
  when Azure is enabled. `delete NAME` deletes the VM, disk, interface, security group and address
  and only then the registry entry and the fleet key.

### Changed

- The web edge builds its address, security group and interface from the same Azure request
  bodies as cloud hosts (`providers::azure::public`), and its delete now follows each Azure
  operation to its end before confirming the resource is gone.

### Fixed

- **A throttled Cost Management query no longer leaves the Azure balance unread:** on HTTP 429
  the reader holds the longest wait Cost Management names (`x-ms-ratelimit-…-retry-after` or
  `Retry-After`), announced as a wait, and asks again; a 429 naming no wait is refused with its
  body. Before, one throttled read made every Azure create refuse for an unknown balance.
- **An Azure machine size is priced as a VM, not as its Cloud Services (classic) role:** the
  Retail Prices API lists both under the same `armSkuName`, and the credit gate took the higher
  one (5.696 instead of 2.752 USD an hour for `Standard_D64as_v5`). Only products named
  `Virtual Machines …` are quoted now.
