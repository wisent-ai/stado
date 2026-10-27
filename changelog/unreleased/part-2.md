# Changelog Unreleased (part 2)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **`stado release policy portable-root PRODUCT [--check] [--json]`:** an install root spelled under
  one target's home while other targets' homes differ is written as the same path under `{home}` in
  one verified registry write, so every target installs under its own home; `--check` writes nothing,
  a root already under `{home}` is reported, one under no target's home is refused. `registry
  doctor`'s `foreign-install-root` finding and the evacuation's refusal name it, `placement evacuate
  --portable-roots` runs it for the destination before anything moves (report `portable_roots`), and
  Stado Desktop runs every form. `release status --limit` takes a non-zero count by its type.
- **`stado placement evacuate --repin-schedules`:** the schedules pinned to the host being emptied
  are read before anything moves (a schedule store that does not answer is one more refusal) and,
  once the host is verified empty, each is pinned to the destination through the store's
  compare-and-swap edit, so a schedule changed meanwhile is never overwritten. The report carries
  `repinned_schedules`.
- **`stado placement evacuate --drop-release-targets`:** each release-control product that rolls out
  to the emptied host stops being released there, through the same verified write as `stado release
  policy remove-target`. A product released only to that host is refused before anything moves,
  unless the run stands the destination by for it; the report carries `release_targets_dropped`.
- **`stado placement holds` names what keeps acting on a host after its services leave:** the
  release-control products that roll out to it (`rolled_out_by`, with the `remove-target` command for
  an emptied host) and the schedules pinned to it (`pinned_schedules`, or
  `pinned_schedules_unread` with the store's cause). Without a host, each row says whether that host
  is `emptied`.
- **An evacuation states its verified end:** `verified_emptied` in the report, and the text form
  ends with `the registry places no profile and no role of the host Stado process on A now`.
- **`stado registry doctor` names schedules pinned nowhere:** `schedule-pinned-nowhere` for a live
  schedule whose pin no registry target names (by name or `<kind>-<hostname>` identity, the rule the
  claim path uses), `ambiguous-schedule-pin` and `unreadable-schedules`. Pins written by
  `evacuate --repin-schedules` take the consumer identity `submit --pinned-host` writes. It also
  names `profile-lacks-catalog-stores` for a profile a move would refuse, and `stado placement
  profile sync-state --all` closes every such finding in one command, naming any profile it could not
  sync.
- **A stopped evacuation resumes when run again:** the stop names it, an authority the destination
  already holds is reported as `already_authority` instead of refusing the rerun, and an emptied host
  is accepted when schedules or release targets are still to hand over. `placement history` keeps the
  run's whole report with its reason and prints plain lines. The schedule hand-over reads the store
  back and names any schedule still pinned to the emptied host; a release target dropped meanwhile is
  passed over.
- **The Desktop operation check covers options:** `Stado --operation-paths` prints each operation's
  field options, and `tests/desktop_api` names every option the command's help does not take.

### Fixed

- **A host's capacity publication says what was not measured instead of a zero:** a closed admission
  published `free_vram_gb` 0 for cards nobody read; it is now null. The disk gate's free space,
  staging space and largest pending directory are published as measured (GiB through one unit
  constant), each unmeasured figure null with its cause in `<name>_unread`, and nothing pending is
  null. The agent publishes its volume's `disk_total_bytes`, `disk_free_bytes` and, when it cannot
  read it, `disk_unread`; builder placement judges a build's scratch against those bytes instead of
  rebuilding them from a rounded GiB figure and a percentage.
  A failed disk write probe now publishes the operating system's reason in
  `home_write_probe_error` and logs it, instead of a bare `home_write_probe_ok: false`. Every
  published disk cause names the fleet volume rather than its local path, as the capacity
  diagnostics promise, and the disk gate's log lines state their figures in GiB. All disk readings
  of the agent, the janitor and the build worker go through the disk-full rule's one volume reader.
  The fleet-volume probe also checks removal explicitly; a failed unlink refuses admission with
  its own cause instead of being discarded by temporary-file cleanup.
- **Staging leaves a RAM-backed `/tmp` for a mount chosen by its device:** the agent passed over
  mounts whose filesystem name was on a fixed list, so an unlisted RAM or network filesystem could
  be chosen. A candidate is now a mount whose source is a block device. Mount paths are decoded
  before use; an unreadable device is skipped with its cause. The write probe creates its own
  file exclusively and refuses a candidate if writing or removing it fails. An unidentified
  medium ranks after identified disks and is never reported as an HDD.
  Partition media comes from the kernel's device hierarchy rather than a partition-name regex
  or trailing-digit removal, so different block-device naming schemes use the same reading.
  Explicit staging paths retain their whitespace and use path-component comparisons, so a
  directory whose name only starts with `tmp` is not treated as a child of `/tmp`.
  An empty or non-UTF-8 filesystem-type answer is reported as an unreadable `/tmp`, not as
  proof that staging is already disk-backed.
  Root-owned staging handover corrects both UID and GID and reads them back, refusing a candidate
  whose ownership cannot be observed rather than accepting an unchanged group.
  An empty staging-user override, an account row without a name, or an effective UID absent
  from the account table is refused with its cause instead of substituting an unknown owner.
  A non-root agent refuses another account's UID instead of ignoring its staging selection.
  The resolved account is reused across candidates rather than rereading the account table.
  Mount-table fields use space and tab separators, preserving other whitespace inside names.
  Kernel-escaped hash marks in device sources are decoded alongside whitespace and backslashes;
  each escape is interpreted once.
  The startup redirect covers ramfs as well as tmpfs, retaining the observed filesystem name
  in diagnostics instead of labelling every RAM-backed /tmp as tmpfs.
  Repeated mount points use the final source for medium lookup; overwritten source fields
  are not decoded and allocated only to be discarded.
- **Pending staging size must be measured completely:** unreadable entries invalidate the total
  and refuse admission instead of being skipped and counted as spare room. The publication
  carries `largest_pending_raw_dir_gb_unread`; an absent pending root remains nothing pending.
  Tree measurements do not traverse symbolic links and reject byte counts that cannot fit.
  A disk-gate refusal also carries its measured cause as admission_detail, so host gates and
  Desktop do not stop at the generic disk_gate_refused label.
  Write, flush, sync and removal failures name their own operation rather than combining a
  persistence failure with a write failure.
  Fleet and staging probes explicitly attempt removal even after an earlier operation fails,
  preserving both causes if the failed probe also cannot be removed.
  CLI host gates and Desktop also retain a supplied refusal detail when the publication has no
  reason label, rather than discarding that cause for a generic busy-or-gated message.
- **Reviewed resource plans expand dependencies without duplicate staging lists:** rationalization
  selection adds only previously unseen dependency identities in place, rather than allocating
  cloned dependency vectors on every closure pass. Approval and irreversible-action rules remain.
  Apply, verify and restore also preserve a journal-lock release failure beside an operation's
  own failure, retaining the original class instead of silently discarding the cleanup cause.
  Releasing an already-released journal lease by the same owner is idempotent: the original
  release timestamp remains intact and no redundant compare-and-swap is attempted.
  Lease renewal and release enforce the same operation-identity validation as acquisition,
  refusing malformed identities before composing or reading a storage path.
  Operation-state transitions recheck their operation identity before persistence, so a callback
  cannot store a state under a different operation's journal.
  A journal failure while recording a failed preflight, action or rollback no longer hides the
  original resource failure: both causes are reported with action and operation identities,
  preserving the original failure classification and structured details.
- **Autonomous mutations refuse invalid cost estimates:** NaN, infinity and negative estimates
  cannot bypass a single-action ceiling through floating-point comparison. The authorization
  reason names the invalid estimate; read-only actions and absent estimates keep their behavior.
- **Autonomy time bounds fail with a cause instead of panicking:** decision TTLs and
  circuit-breaker cooldowns validate the duration representation as well as positivity.
  Decisions, plans, schedule windows, leases and circuit updates check calendar arithmetic,
  reporting the field, supplied seconds and failed expiry or lookback calculation before
  persisting an invalid timestamp. Advice computes its shared expiry once per pass, and
  schedule-marker age comparison no longer narrows an unsigned TTL to signed seconds.
- **Advice identity encodes field boundaries:** new recommendation IDs hash a typed JSON
  tuple rather than pipe-joined strings, so a delimiter in one identity component cannot
  stand for a boundary between components. Encoding streams directly into SHA-256 without
  an intermediate payload allocation; serialization failures propagate before publication.
  Existing immutable decisions keep their identities.
- **Unjudged advice is no longer silent:** manual and scheduled autonomy passes emit each
  recorded unjudged cause independently of newly published recommendations, including a
  pass that could judge no resource. The resource identity and original cause stay visible.
- **Policy documentation distinguishes omitted mode from missing policy:** report remains
  the mode default inside a written policy, but no policy is synthesized without the
  operator's idle, freshness and safety inputs. Ordinary queue matching is unchanged.
  The absent-policy refusal also names the parser's required `--file` option rather than
  suggesting an unsupported positional file argument.
- **Network advice retains unresolved dependency evidence:** a declared dependency absent
  from the inventory makes network advice unjudged, naming the resource, dependency and
  snapshot, instead of silently dropping the edge. Other recommendation kinds continue.
  Normalization preserves already-canonical dependency declarations instead of dropping
  or reinterpreting them as native aliases, moving their owned strings without copying.
  Unresolved native candidates remain source evidence, not fabricated resource nodes.
- **Commitment advice refuses partial portfolio prices:** each observed instance needs a
  finite, non-negative hourly price and the sum must remain finite. A missing price,
  invalid value or overflow records the instance and cause under unjudged instead of
  understating the observed portfolio. Financial-commitment authorization is unchanged.
- **Retention diagnostics survive warning-only passes:** lifecycle output includes the
  entire nonempty summary, including unmeasured object names and cap state even if nothing
  was deleted. Retention limits and deletion decisions are unchanged.
- **Retention quotas use checked arithmetic:** day-to-second conversions fail by field
  before cleanup starts, byte limits use measured remaining budget, and an unrepresentable
  deletion-byte total refuses that request before mutation rather than saturating.
- **Retention errors retain partial progress:** validation, listing and delete failures
  name the operation, target, cause and complete acknowledged summary through the same
  manual/scheduled cycle logger, then propagate the original storage error unchanged.
  Failed delete requests report an unconfirmed outcome, not an assumed rollback.
- **Storage advice requires unused attachment evidence:** AWS volumes must be available,
  GCP's disk probe must report unattached, and Azure disks must report Unattached before
  age can justify storage-lifecycle advice. A missing workload label alone no longer
  makes an attached disk look unused; missing or malformed attachment observations
  retain the field and observed value under unjudged. Advice remains recommendation-only.
- **Network lookup reuses the inventory's ordering without copying it:** one lazy
  ordering check enables lower-bound searches for ordered snapshots; unordered input
  retains first-match lookup. Disabled egress advice and empty dependency sets do not
  trigger an ordering scan; missing-dependency diagnostics remain unchanged.
- **Dependency normalization borrows alias identities:** native references, names and
  target IDs are indexed by reference, with owned strings only for stored edges.
  Empty dependency sets skip the index; canonical declarations and precedence are unchanged.
- **Rightsizing retains invalid utilization evidence:** unusable selected metrics name their key and value under unjudged.
  Validated peaks are reused in the target; missing dimensions remain unknown and alias precedence is unchanged.
- **Qualification distinguishes missing evidence from a running build:** a successful run whose
  required platform receipt is absent reports awaiting_tests with the platform and receipt path.
  A real failed build retains its build cause instead of an unrelated incomplete-test explanation;
  an explicit failed worker receipt remains authoritative in CLI and Desktop handoff results.
- **A GPU row the driver printed is never dropped:** the agent skipped any `nvidia-smi` card row it
  could not read, so a card the host has vanished from admission without a word. Such a row now
  refuses the reading, naming the row and the field (`nvidia-smi's memory.free "[N/A]" for GPU-… is
  not a MiB count`), which the agent publishes as its accelerator-read failure.
- **A job's GPU footprint is read like the holders table and a failed read is said:** the per-job
  reading answered `-1` for an unreadable driver and dropped compute-app rows it could not parse;
  it now reads rows with the holders' parser, a host without a driver holds no GPU memory, and a
  driver that fails is logged against the job while its peak keeps what was read. A verification
  command whose secret environment cannot be resolved is no longer run without it, and every
  verification failure is logged with its cause.
- **File digests are taken one way:** release archive verification, web artifacts, `stado storage
  archive`, database dumps, backup audits, reconcile evidence and machine sources each streamed a
  file through SHA-256 with their own buffer and their own error wording; they now share one reader
  whose error names the path that could not be opened or read.
- **An unnamed run is named by what it runs:** its readable name was built from `-m`, `--model` and
  `--task` flags, so every other kind of command got only a job count. It is now the commands'
  distinct programs and the job count (`python:40jobs`); `WC_RUN_NAME` still names it outright.
- **`stado host local repair` says where a store root was looked for:** `store_root unresolved`
  now names `WC_LOCAL_STORAGE_PATH` and `storage.local.path` in the config it read (and the backup
  pair), and a root that cannot be resolved, or a variable that is not UTF-8, is refused with its
  cause instead of being read as absent.
- **A loopback listener is recognised and addressed one way:** the Weles admission channel accepted
  only the spellings `127.0.0.1`, `::1` and `localhost`, so a declared `http://127.0.0.2:…` loopback
  address was refused as remote; it now asks the shared origin check, which knows every loopback
  address. Placement eviction, web deploy records, ingress verification, the storage reconcile's
  served-store read and the Weles channel build their loopback URL through one helper. A public
  origin's `upstream` is judged by the same check (any loopback address or `localhost`, explicit
  port), and its `name` no longer carries an undeclared length cap.
- **Admission, the memory pass and `space report` read memory one way:** the agent admitted jobs
  against its own macOS sum of free, inactive, speculative and purgeable pages while the memory pass
  and `space report` used the kernel's `kern.memorystatus_level`, so a host could be admitted for
  memory the pass counted as taken. All three now read `MemAvailable` on Linux and the kernel's level
  on macOS through one reader. `space report` folds the rows it reads from a host through the same
  parsers, reads the page size from the `vm_stat` banner, and names every figure it could not read
  in `memory.unread` instead of leaving it silently null; a malformed `vm.swapusage`, `vm_stat` row
  or meminfo figure is now that cause rather than a missing number. A Stado object read names a
  version header that is not visible ASCII instead of calling it omitted.
  `stado runner diagnostics` read its own shell arithmetic of the same figures (truncating the
  kernel's level to whole hundredths of memory); it now prints the same memory section and folds it
  through the same reader, and its absent fields are `null` instead of `-`.
  Capacity publications and `stado host gates` JSON carry `memory_available_gb` and
  `memory_total_gb` unrounded; the text line and Desktop round them where they show them, in GiB.
- **Every archive Stado builds stamps its headers one way:** a release build's staged output, a web
  artifact, a checkout snapshot, its provenance records and `stado storage archive` each spelled the
  epoch, the root owner and their modes on their own, and a staged build output kept the builder's
  umask, so one commit could package to different bytes on two builders. All five now take the
  epoch, the root owner and the two `git archive` modes (owner-executable or not) from one
  declaration. `stado storage archive OUTPUT` with a bare file name no longer fails on the empty
  parent path, and a staged file outside its stage entry is refused by name instead of archived at
  the stage root.
- **`stado doctor` names what is wrong with the release it reaches:** a manifest that did not match
  was reported as `has invalid identity or archive digest`; the check now names the field and what
  it holds. A system resolver that cannot place a tailnet release origin is reported with its own
  error instead of as an empty answer, and the port the route is judged on is the API URL's own.
- **The workstation watchdog reads through Stado:** its capacity probe ran `gcloud storage ls` on a
  bucket path no host can reach; it now runs the collecting binary's own `stado capacity list
  --json`. Volumes are reported as the disk-full rule reads them (bytes, percent used, headroom,
  `full`) instead of a rounded GiB copy of `shutil.disk_usage`, command output is kept whole, and a
  missing program is named as such. The payload is `wisent-box-diagnostics-v2`.
- **A host's health beacon is named by the store that holds it:** its report and its refusals called
  it `gs://<bucket>/…` and its registry "the GCS registry" whatever backend held them, sending the
  reader to a bucket that is not where the beacon lives. They now name the Stado object and the
  canonical registry.
- **A registry write is what every host reads next, the writer included:** `resolver snapshot`
  answers the authority's running resolver's held registry, and every host's reads take its own
  resolver's held registry first; both advanced only on their refresh tick, so a second write within
  one interval read the version before the first and was refused as a conflict, even from the same
  command. A compare-and-swap now asks the resolver of the host whose store it landed in, and then
  that of the host that asked for it, to read at once and waits until each holds the written store
  version; a resolver that does not follow is said on stderr and the write stands.
- **`stado registry set` no longer refuses a consumer's `grants` by a spelled release number:** the
  write was gated on every host declaring a Stado of a release long past; every host the registry
  declares runs a later one, so the gate and its version comparison are removed, and the receipt line
  prints a non-text value as its JSON without a fallback.
- **`stado release policy apply` refuses an install root written under one target's home** while
  another target's home differs, naming the hosts and the portable `{home}/<rest>` root; `registry
  doctor`'s `foreign-install-root` finding reads the same rule.
- **`stado product changelog --spill` no longer adds files to the crowded range folder:** spilled
  parts are written as `changelog/unreleased/part-N.md`, and a part an older spill wrote as
  `changelog/unreleased-part-N.md` is still read and gathered by the version bump.
- **A stream declared without `--gpu-uuid` no longer encodes on an invented device 0:** the Sunshine
  unit names `CUDA_VISIBLE_DEVICES` only for a declared board, so the encoder otherwise opens the
  driver's own default, as `stream declare` promises.
- **Text output prints report fields as values, not raw JSON:** placement history, standby `--check`,
  move-role, the authority move, placement transactions, ci status, repair, release rollout status,
  secrets retire, Weles activity, the resolver readiness report, `inference routes show`, the ci main
  record, `storage fence status`, the overview's fleet line, service recoveries, contributions and
  the forge, handoff and build-approval refusals print through the shared field reader: text bare,
  an absent field as `not reported`.
- **`stado service verify --json` no longer prints `[]` when its findings cannot be written:** the
  serialisation error is the refusal, and the text table takes its widths from the content.
- **`stado web remove` says what each receipt said** (`no-unit` for a hostname-only product), and
  **`stado credentials seed enrol`** names an absent seed row instead of calling it unreadable.
- **Vast.ai listings carry only the prices the operator stated:** `market auto-list --provider vast`
  takes `--price-disk` and `serve --vast-auto-list` takes `--vast-price-disk`, both refused by name
  when missing; no built-in disk, bandwidth or minimum-chunk value is sent, and the auto-list loop
  watches the configured bucket instead of a fixed one. Stado Desktop's Earning preview passes the
  form's disk price.
- **A machine the Vast.ai account does not hold is refused, not reported:** `market status`,
  `market readiness` (now `vast_refused`, naming the machine and the ids the account holds) and the
  bridge no longer read it as a ready or merely unlisted machine. Box and Vast.ai answers read text
  and booleans only as JSON text and booleans, naming the field Boat or Vast sent in another type.
- **`stado placement evacuate --authority` empties a host whose registry authority no longer
  answers:** every move committed through the authority, so a silent one stood the whole run. The run
  now takes the authority on the destination first and checks and makes every other step through
  it; `--check` refuses naming the silence, and a silent authority whose host also serves the fleet
  object API this host's registry store is refused before anything is promoted. A failure after the
  moves (the read-back that the host is empty, the schedule repin, the release-target drop) names
  what moved and that a rerun resumes.
- **The fleet object API can move off the registry authority's host:** its store holds the registry
  that authority writes, so `placement move-role` fenced the very store its registry commit needed
  and undid itself. `stado placement move-role --with-authority` (run on the destination) reads the
  registry under the fence, switches the route and names the destination the authority in one
  document, commits it at the authority through that fence by the move's own transaction (`registry
  push --fence-transaction`, the one writer a fence admits), and writes it into the copied store
  through the destination's own object API and into its copy, then asks the destination's resolver to
  read the registry at once (SIGUSR1) and waits until it holds the new generation; a source holding
  the authority without the flag is refused naming it, and `placement evacuate --authority` moves the
  first role of such a host this way. `placement holds` with no host also lists each host's pinned
  schedules and store write fence, `placement holds HOST` says whether the host's store is fenced
  (and by which transaction), `stado resolver follow [--generation N]` asks this host's resolver to read now (after
  the resolver it reads from follows its own registry) and waits for it, and the evacuation says each
  step on stderr as it starts, waits after each move until this
  host's resolver holds the registry the authority serves, and refuses a release-controlled product
  whose install root would send the destination outside its home.
