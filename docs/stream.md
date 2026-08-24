# Interactive streaming

How do you use a fleet GPU interactively — for a desktop, a renderer, or a
game — from your own machine? A board cannot be borrowed over a network: the
process that renders has to run on the machine that owns the card, and only
the frames travel. `stado stream` provisions that renderer on a fleet host and
streams it to a client (Moonlight).

The fleet already places batch work and inference on a GPU host; an
interactive session is the third thing a board can be asked for. Batch jobs
reach the same hosts through provider adapters ([providers](providers.md)),
and model serving through [inference](inference.md). Flag-by-flag listings
live in [cli](cli.md).

## What apply builds

The session is declared per target, because that is the level at which it is
true or false: a host either carries the session or it does not. On a host
that has boards and no monitor, `stream apply` builds:

- an Xorg screen the driver invents (`AllowEmptyInitialConfiguration`), sized
  by the declaration, pinned to one board by PCI bus id;
- a session on it (`openbox`, because something must own the root window and a
  full desktop is not the ask);
- Sunshine, installed from a digest-pinned `.deb`, encoding that screen with
  the board's own encoder;
- two systemd units (`stado-stream-xorg.service`,
  `stado-stream-sunshine.service`), so the pair survives a reboot without a
  display manager and without logging anyone in.

Every host operation runs as fixed script text with the declaration's values
substituted, over the registry SSH channel — no operator words reach a shell.

## Provisioning a session

```bash
stado stream probe gpu-host
stado stream declare gpu-host
stado stream apply gpu-host
```

`probe` is read-only and answers before anything is installed: boards with
their PCI bus ids and UUIDs, driver version, DRM nodes, encoder presence,
whether a display manager already owns the screen, free space on the declared
library volume, and the tailnet address a client would dial.

`declare` writes `targets[TARGET].display_stream` into the canonical registry
— the placement fact lives where every other placement fact lives. Defaults:
2560x1440 at 60 Hz, library at `/mnt/wisent-games`; `--steam` installs Steam
beside the session. It refuses a library on the root volume, a resolution
outside 640..7680, a refresh outside 24..240, and an unpinned Sunshine. The
Sunshine artifact is picked from the host's own distribution, because that is
what decides whether it installs at all; `--sunshine-url` with
`--sunshine-sha256` pins one explicitly (measured, never guessed) for a
distribution this build has no measured digest for.

The board is declared by driver UUID (`--gpu-uuid`; omitted leaves the
driver's default, which is the board the job agent also prefers) and Xorg is
configured by PCI bus id; `apply` resolves one to the other from the probe,
because only the host knows that mapping. On a two-card host, naming the
second board keeps the session and the job agent out of each other's way: the
agent places work on the emptiest card, so a session holding one board pushes
batch work to the other.

`apply` reconciles the host to its declaration and is idempotent — an
installed host reports what it already had. `--provision-library` binds the
declared library directory onto the host's largest disk-backed filesystem when
it would otherwise land on a root volume with no room; without it, such a host
is refused and its mounts are named, because reshaping storage is not
something to do quietly. The line written to `/etc/fstab` carries a
`# stado-stream` tag so purge removes exactly its own.

## Connecting a client

The fleet does not install or launch a client. Moonlight runs on the
operator's own machine, and the pairing PIN comes from it.

```bash
stado stream status gpu-host
stado stream pair gpu-host --pin 1234
```

`status` answers "what is the session doing right now, and where do I point
the client": units, the screen's real size, the board carrying it, bound
ports, paired clients, room left for the library, and the address for
Moonlight.

`pair` hands Moonlight's four-digit PIN to Sunshine's API over the managed
host channel, with no browser involved — the only other route to that API is a
web UI this fleet does not open on an operator's machine. The PIN authorises
exactly one pairing and is the one operator value this surface takes; it is
checked for shape before it is substituted, and the web credentials are
generated on the host and never leave it. `--client` names the paired client
(default `moonlight`).

## Ending a session, and what remains

```bash
stado stream stop gpu-host
stado stream stop gpu-host --purge
```

`stop` stops the session. `--purge` also removes the units and the Xorg
screen, so the host can go back to being headless without a trace beyond the
packages.

Anti-cheat is the one thing no configuration fixes: titles with kernel-level
anti-cheat refuse to run on Linux, and streaming a Linux host does not change
that.
