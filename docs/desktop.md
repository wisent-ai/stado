# Stado Desktop

What does the native macOS console show you, and what does it never do behind
your back? Stado Desktop is an operator's console over the Stado CLI: every
screen reads through `stado ... --json`, every write runs the exact `stado`
invocation the screen quotes, and the app keeps no state of its own — anything
it shows can be checked against the same command in a terminal, word for word.

## Local by default, no session required

The local control plane is the app's default source. The local path requires
no deployment setup, and the active source is visible in Settings — a local
profile reads `Local Stado`, a remote one names its deployment, provider, and
endpoint. The operations console works without a Wisent session; account
sign-in is required only for remote deployment actions. The menu-bar app
reopens the native console rather than sending you to a browser.

The dashboard address is read from the same configuration file every other
reader of the fleet uses (`storage.stado.url`); `127.0.0.1:8765` — this
machine's own host-health API — is a last resort only, because on an operator
laptop its local copy of the store lags the fleet by days. Direct loopback
dashboard access needs no Supabase round trip, and proxied product APIs keep
their credential boundaries.

## What the screens show

The Hosts, Services, and Releases screens are documented with screenshots in
the app's own README under `desktop/StadoDesktop/docs/screenshots/`. Their
shared discipline:

- **Services** separates what is declared from what is running: declared
  units, the fleet as the health beacons report it, and processes no unit
  owns. `running_binary` is a column rather than a detail, and orphan
  processes are a list of their own — both facts were learned the expensive
  way. Beacon-reported states are printed with the beacon's own timestamp,
  so an `active` from a five-day-old beacon reads as five days old; the
  console never re-asks a host to fill the gap.
- **Releases** shows one row per product target with the verdict
  `stado release doctor` reached, desired and observed versions side by
  side, and blockers in the CLI's own words — it reads `verdict`, `failed`,
  and `findings` out of `status --json` and re-derives none of them. A
  host's quarantined digests are listed with the one the registry desires
  first, then newest first, so the digest actually blocking a rollout is not
  buried under refusals that are already history.
- Dashboard state decoding presents the published backend snapshot — worker,
  capacity, job, failure, and onboarding labels reflect what the backend
  recorded, not optimistic placeholders.

The console reads a command's payload before it reads its exit status:
`host gates`, `service converge`, and `release status` each exit non-zero
after printing a complete `--json` answer, and each answer lands on the exact
screen built to show it. The exit status is what a failed read reports only
when there is no payload to read. When an invocation truly fails, the error
pane carries the CLI's own sentence rather than a category of it — "command
failed" would only send you to a terminal to run the command the console just
ran.

## Add a Machine

The Hosts screen carries an Add a Machine path, reachable from the context
bar and from the empty registry state. The window opens on the ways in rather
than on one of them: the list comes from `stado fleet methods --json`, so a
method this fleet's registry catalog forbids is shown disabled, naming the
field that forbade it, instead of being missing or dead.

- **Invite** mints a code, shows it once with the single line to send to
  whoever has the machine, then waits. The wait survives quitting the app;
  reopening the window says which machine is expected, what it reported when
  it answers, and offers approve or reject.
- **Adopt** takes an address and installs the fleet's public key over a
  session the control plane can already open — and says plainly that no
  password can be typed into the window, because the process opening that
  session has no terminal.
- **Join** and **Declare** each state what the operator has to do, with
  pending and approve behind Join.
- The hand-carried key path remains for the machine nobody can reach: name,
  key minting with the public half and its `authorized_keys` line to carry,
  the SSH address, verified enrollment, and the channel and agent proofs.
  The two proofs run for a machine added by any method.

Every step runs one allowlisted `fleet` or `host` command through the
dashboard's `POST /api/operator/run` argv bridge — an argv array checked
against a closed family allowlist, with the mutation confirmation the
console's own operator page sends. No command string is ever assembled, and
there is no second transport. Enrollment progress belongs to the application
rather than to the window, because adding a machine spans a walk to another
computer: closing the window mid-walk does not cost the minted key. An
enrollment that fails says whether it never reached the machine, or reached
it and rolled its own entry back. The command-line equivalent of the whole
flow is [add-your-machine](add-your-machine.md).

## What the app never does

- It never invents state: no snapshot of its own, no optimistic labels, no
  fallback list of enrollment methods when the control plane has not
  answered one.
- It never edits the registry directly. `GET /api/registry.json`
  deliberately returns three whitelisted fields per target; routing and SSH
  material stay inside the registry document and are never sent to an
  operator client. The operator writes are a whitelisted policy patch and
  one recorded job rerun.
- It never assembles shell strings; every mutation is an argv array through
  the authenticated operator bridge.
- It never transmits a private key: the channel key's private half stays in
  the operator's credential store, and only the public line reaches the
  machine.
- It never registers a duplicate: enrollment refuses a name the registry or
  the capacity store already knows.
