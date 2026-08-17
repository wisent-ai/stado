# Add your own machine

For the person whose computer is being added to a fleet somebody else runs. Two steps happen on your machine; everything else happens on the control plane. If you are the operator, use [Onboard another machine](onboarding.md#onboard-another-machine) instead — this page deliberately repeats none of it.

## What you do on your machine

**1. Let it accept a key.** On macOS, enable **System Settings › General › Sharing › Remote Login**. On Linux, make sure an SSH server is running and accepts public-key authentication. Stado never asks for your password and never uses one: the channel is key-only.

**2. Install the key the operator sends you.** They run `stado fleet key generate <name>` and send you the one public-key line it prints. Append that exact line to your `authorized_keys`:

```bash
mkdir -p ~/.ssh && chmod 700 ~/.ssh
printf '%s\n' '<the line the operator sent you>' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

The key is generated for your machine alone and its private half never leaves the operator's credential store. Nothing else is installed by you, and nothing needs a Stado binary on your side yet.

**3. Tell the operator how to reach you.** Give them a destination they can actually open, in `user@host` form. On the same network, your machine's `.local` name works — `you@your-mac.local`. A tailnet or VPN name works the same way, and so does a routable address; Stado stores whichever you give and requires no particular kind. A `.local` name limits the operator to working from inside your network, but not the health reporting: that always travels outward from your machine.

## What the operator does

One command that probes before it writes: it reads your machine's `hostname` and `uname` over the key you installed, records those, installs the agent, and rolls the entry back if the install fails. Nothing is registered about your machine that was not read from it, and a machine that could not be reached is never left half-registered. They can also do it from Stado Desktop's **Fleet › Hosts › Add a Machine** sheet, which walks the same steps and shows you the key line to paste.

If they cannot reach you but you can reach the fleet's store, the direction flips: you run `stado fleet join` on your machine and they approve the request.

Stado's own footprint on your machine is a per-user agent and health beacon — a launchd or systemd-user service under your account, with state under `~/.stado`. It does not require administrator rights, install system-wide services, or supply your workload's runtimes.

## Confirming your machine reports

Ask the operator for the output of:

```bash
stado registry beacon-age
```

Your machine should have a row with a fresh timestamp. A machine that never reported is listed too, with no beacon, so silence is visible rather than absent. `stado host health <name>` gives the same machine's disk, services, and log tail.

On your machine, `stado registry self --name-only` prints which registry target it is — the answer to "is this box actually the one they registered".

## Leaving

Ask the operator to remove the registry entry, then, if you also installed Stado locally, follow [Uninstall and local reset](onboarding.md#uninstall-and-local-reset). Removing your public key from `~/.ssh/authorized_keys` closes the channel from your side at any time.
