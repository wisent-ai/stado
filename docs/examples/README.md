# Examples — stado in practice

Executable examples as **plain command sequences** — the commands
themselves, per the Wisent PRODUCT guidelines (CLI design contract +
examples requirement). Each script runs end-to-end locally.

## Command surfaces

- **First run** — `config init`, `config validate`, `doctor`
- **Work** — `submit`, `job watch`, `results`, `job rerun`
- **Secrets** — `secrets put / get / ls / rm / doctor`
- **Fleet** — `fleet enroll`, `fleet key generate / check`,
  `registry beacon-age`, `host ping`, `host uptime`, `service list`,
  `service status`
- **Queue** — `queue status / pause / drain / resume`

## Index

1. [`onboarding-local-job.sh`](onboarding-local-job.sh) —
   from zero to one completed local job: config init, validate, doctor,
   submit, watch, download.
2. [`secrets-store-and-read.sh`](secrets-store-and-read.sh) —
   store via stdin, read back, list.
3. [`fleet-health-check.sh`](fleet-health-check.sh) —
   beacon age, host ping, service list — fleet truth without ssh.
4. [`queue-maintenance.sh`](queue-maintenance.sh) —
   pause, drain, resume — maintenance without cancelling work.
5. [`fleet/add-remove-host.sh`](fleet/add-remove-host.sh) —
   declare a device with `registry host add` (`--ssh` and
   `--release-platform` both required), remove it via
   pull → edit → validate → push. Ends net-zero; verified on the real
   registry.
6. [`fleet/onboard-host.sh`](fleet/onboard-host.sh) —
   bring a device to reporting life the verified way: `fleet enroll
   --bootstrap` (probes hostname and platform before it writes), `fleet key
   check`, skarbiec grants (`stado-local-agent`,
   `stado-host-health-beacon`), host recover, beacon-age as proof. The public
   key it needs in the target's `authorized_keys` comes from
   `stado fleet key generate` — see
   [Add your own machine](../add-your-machine.md).

## Providers (opt-in backends, per user)

Each provider lights up the same way: credentials into YOUR skarbiec,
provider flipped on in YOUR config, one verify command. Credentials come
from your env, never inline.

7. [`providers/enable-azure.sh`](providers/enable-azure.sh) —
   `wisent-azure-billing-sp` (tenant_id, client_id, client_secret), then
   `stado azure`.
8. [`providers/enable-gcp.sh`](providers/enable-gcp.sh) —
   `stado-gcp` (service_account_json), then `stado doctor`.
9. [`providers/enable-aws.sh`](providers/enable-aws.sh) —
   `stado-aws` (access_key_id, secret_access_key), then
   `config validate`. Verified end-to-end on a scratch config.
10. [`providers/enable-vast.sh`](providers/enable-vast.sh) —
    `stado-vast` (api_key), then `stado vast list`.

## Template for a new example

Per skarbiec's `PRODUCT.md`: the example IS the commands a user would
type, in order — `set -eu`, a usage comment, env for values, nothing
else. Verification is itself a printed command. Every line must be
copy-paste runnable.
