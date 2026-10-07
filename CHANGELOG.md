# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.46](changelog/0.23.42-0.23.46.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A placement state can live under a host's work root:** every `placement_profiles[].state[].path` was relative to `$HOME`, so a store a profile moves could only sit on each host's home volume, even where the registry declares a larger `work_root` (75879314). A state now takes `root: work` beside `path` and `required`; a move resolves it under the source's `targets[].work_root` when it reads and under the destination's when it writes, rolls back and cleans its backup there, and prints it as `<work root>/<path>`. `root` absent or `home` keeps `$HOME`. A move whose source or destination declares no `work_root` for a work-rooted state is refused before the source is fenced: `<host>: state <path> is kept under the work root, and the registry declares none for <host>; declare targets.<host>.work_root before moving it there` (config). The path rule reads `must be a clean path relative to its root ($HOME, or the host's work_root with root: work)`.

- **A placement state can be a whole directory:** `tree: true` on a `placement_profiles[].state[]` entry moves the directory whole instead of reading it into one in-memory file snapshot, so an object store tree can be placed (75879314). The fenced source's tree is copied with `rsync -a --delete` over the source's Stado SSH route into a staging tree on the machine running the move (under its declared work root, or `~/.stado/placement/<transaction>`), then into `<path>.placement-<transaction>.stage` beside the destination path, and one rename puts it in place; the destination's previous tree becomes `<path>.pre-stado-placement-<transaction>`, which a rollback renames back and a committed move removes. `deploy::host_delivery::sync_directory` is that copy in either direction. Refusals: `<path> is not a directory`, `placement backup already exists: …`, `required state … disappeared after fencing`, `<host>: copying the tree <path> failed: <rsync's last line>`, and a staging copy that cannot be removed after a successful install is named with its path.

- **A declared service can run a container image pinned by digest:** a declaration's `source.artifact` may be `oci://<repository>@sha256:<digest>` with `source.sha256` the same digest; `stado service deploy <name>` then installs nothing and starts `run.program` (the container runtime, such as the absolute path of docker or podman) with `run.args`, which must name that image. This is the first step of moving the inference plane's digest-pinned vLLM deployment onto the service declaration contract (fe7466c9). Refusals: `<name>: source … must name the image by the digest the declaration pins (@sha256:…)`, `<name>: a container image source needs run.program, the container runtime that starts it …`, `<name>: run.args never names the image …, so the runtime would start something the declaration does not pin` (config).

- GPU and cloud compute vendors are compute providers: `arkane`, `crusoe`,
  `cudo`, `hyperstack`, `lambda`, `latitude`, `nebius`, `oblivus`, `oracle`,
  `runpod`, `salad`, `scaleway`, `voltage-park` and `vultr` may be named in
  `providers`. The coordinator dispatches agent machines on them, reaps dead
  ones and counts them against `config/quotas.json`; `stado instances list`,
  `stado doctor` and the resource inventory read their machines. Each reads
  its credential from the Skarbiec item tagged `stado:role:cloud-<provider>`
  and its settings from `<provider>.*`; `stado config validate` refuses a
  missing required setting and any `cloud-<provider>` role in a workload
  grant. Arkane Cloud's deploy API takes no startup script, so Arkane
  machines are listed, read and released but never dispatched. On RunPod and
  SaladCloud the agent runs in a container and takes its name from
  `STADO_WORKER_NAME`. `stado doctor` names the missing `config/quotas.json`
  section of a provider that has no quota API.
