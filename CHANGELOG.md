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
