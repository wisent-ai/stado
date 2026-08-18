# 2026-08-17 — the GPU host the fleet could not see, and the credential chain behind it

One question ("can these GPUs be used for gaming") walked into a chain of
failures that all shared one shape: **a declaration nothing checked against the
world.** This records the chain, the measurements, and what is still open.

## What was wrong, in the order it was found

| # | State | Evidence |
|---|---|---|
| 1 | `training.models_dir` named `/mnt/wd16tb/stado/training` on the RTX host | the 16 TB disk is gone: `lsblk` shows one 3.6 TB NVMe and a 0-byte `sda`, fstab mounts it `nofail`, so the path was an empty directory on the 100 GiB root volume with 12 GiB free |
| 2 | `audit-registry-declarations.py` refused every invocation | its mirror of `capabilities::DECLARED_FIELDS` lacked `gpu_power_limit_watts`, so the drift guard aborted — the one runnable check for declarations the world contradicts had been failing closed |
| 3 | `training` and `transcript_lake` read as unread declarations | both are honoured by `transcript-label-trainer`; neither was catalogued |
| 4 | The fleet's only GPU host published no capacity object at all | `wisent-agent.service` was `active` while the binary restarted its own loop every 10 s with `agent loop failed: File exists (os error 17)` |
| 5 | Three symlinks pointed into the removed disk | `~/.stado/local-backup`, `~/.cargo`, `~/.rustup`, plus `~/.cache/huggingface`; `mkdir` answers EEXIST for a dangling symlink, so every consumer failed with a message about a file existing and none named the link |
| 6 | The agent measured one accelerator on a two-card host | two RTX PRO 6000 Blackwell boards, 97887 MiB each; with a Vast renter holding 60 GiB on card 0 the host advertised `free_vram_gb: 35` while an untouched 95 GiB board sat beside it, and `slots: 2` admitted two jobs against card 0 with nothing setting `CUDA_VISIBLE_DEVICES` |
| 7 | The renter gate could not evaluate on the machine that is rented | `stado agent` read `stado-vast/api_key` as `stado-control-plane`, whose bearer a worker does not hold and must not |
| 8 | `service_directory` changed without advancing its generation | `stado-object-api` for `operator-host` was corrected from its own adapter (18776) to the object API that host runs (18765); `cli/resolver.rs::refresh` refused the change, the cache went stale, the adapter died, and every `stado` command on that host failed with `registry store unreachable` |
| 9 | The vault holds no `stado-vast` item at all | `token-mint` refuses the capability with "capability names a missing item"; the broker's 403 for a field of a nonexistent item is what made this look like an authorization problem |
| 10 | `skarbiec` on the control plane ignored `--token-file` | that build predates the flag and mints a random bearer instead, returning it in stdout; a rotation that discards that stdout leaves the grant on a token no host holds — which is what happened, and what recovering it required |
| 11 | The holder count for a shared bearer was measured from one convention | the RTX host declares it in an env file (`WC_AGENT_SKARBIEC_TOKEN_FILE`), the mac mini in `~/.config/stado/config.json` (`agent.skarbiec.token_file`); a probe that looked only for env files reported one holder when there were two |

## What is fixed, with the measurement that proves it

- **Training artifacts** land on `/mnt/wisent-training/stado/training`, a mount
  point bound to the host's only large filesystem: 3.2 TB available, write probe
  passes. Declared in the canonical registry and defaulted in the one publisher
  (`transcript-label-trainer/scripts/register-placement.sh`).
- **The audit runs again** and reports neither `training` nor `transcript_lake`;
  its object reads no longer die on a connection reset.
- **The agent runs.** `init: JobStorage done`, loop iterating, and
  `capacity/local-ubuntu-server.json` exists for the first time.
- **Both boards are measured.** The broadcast carries `gpu_cards: 2`,
  `gpu_free_vram_gb_per_card`, `free_vram_gb` as the emptiest card, and
  `nvidia-rtx-pro-6000: 2`; each admitted job is pinned to the board it was
  admitted against.
- **Caches and staging** moved off the removed disk to `/mnt/wisent-cache` and
  `/mnt/wisent-staging`, both on the 3.2 TB filesystem.
- **The Vast key is read through the host's own grant** when no control-plane
  bearer exists.
- **The directory generation** is settled by
  `scripts/advance-service-directory-generation.py`, idempotent by content digest.
- **The shared agent bearer is consistent again**: the vault records the bearer
  both holders hold, `stado-huggingface#token` answers HTTP 200 from the RTX
  host, and the mini's copy hashes to the vault's record. The control plane now
  runs a `skarbiec` that honours `--token-file`, proven against a scratch vault
  before the swap.
- **`pinned_only` is declared** on the rented host, so fleet work does not wander
  onto a renter's card while the gate cannot evaluate. Undo:
  `RETIRE_PIN=1 python3 scripts/pin-rented-gpu-host.py`.

## Still open

**The renter gate needs a Vast.ai API key in Skarbiec.** No item matching `vast`
exists in this vault, so the capability cannot be minted and the bridge cannot
authenticate. That value exists only in the operator's Vast account. Once it is
set (`skarbiec set stado-vast --type api-key api_key=…`), add the capability with
`scripts/grant-consumer-field-read.py stado-local-agent stado-vast api_key` — no
rotation needed, the bearer is reproducible now — and retire `pinned_only`.

**The 16 TB disk is physically absent.** fstab keeps it `nofail`, so the host
boots without it; nothing else depends on it any more.

**`operator-host`'s resolver serves a stale registry and restarts constantly.**
`launchctl print` records `runs = 5019` with `last exit code = 78: EX_CONFIG`, and
the unit produced no log line between 00:18 and 00:47 while cycling, so the
failure happens before the binary writes anything. Run in the foreground with the
unit's own environment it starts cleanly — `api=127.0.0.1:17600 adapters=6` — and
resolves `stado-object-api` at **generation 8**, while the canonical document is
at 12: its bootstrap source is this host's local object API (`:18765`), whose
registry copy is weeks behind. Two consequences worth naming: anything resolved
through this host can point at endpoints the fleet has already moved, and every
`stado` command here fails with `registry store unreachable` whenever the adapter
is between restarts, because the configured store URL *is* that adapter. The
source already names half of this contradiction in `cli/resolver.rs::serve` ("a
resolver that must serve the very address it reads the registry through cannot
start in either order"), with a comment counting 641 restarts. This is older than
this incident and was not caused by it; the repair belongs with whoever owns that
bootstrap decision, and the measurement above is what it has to explain.

## The rule this chain keeps proving

Every one of these passed a check that modelled the wrong thing: a unit state
that reported `active` for a binary restarting its own loop, a health beacon that
knew the root volume and not the mount a declaration named, a version gate that
measured the command list and not the accelerator count, a bearer hash comparison
that could not see the second holder's configuration. **A declaration must not be
writable without a typed consumer that reads it, and a check must observe the
world the declaration describes — not the document it came from.**
