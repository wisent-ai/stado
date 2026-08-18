# Add your own machine

For the person whose computer is being added to a fleet somebody else runs. Your whole part is one paste and one reply. If you are the operator, use [Onboard another machine](onboarding.md#onboard-another-machine) instead — that page covers all four methods and this one deliberately repeats none of it.

## The fragment the operator sends you

This is the normal way in, and the only one that needs nothing published on the internet. The operator runs `stado fleet invite --offline` on their side and it prints a block between two markers; ask them for it if what you got was something else. It looks like this:

```text
----- 8< ----- stado offline invite for 'your-machine' ----- 8< -----
…a few lines of shell, including exactly one ssh-ed25519 line…
----- 8< ----- end of fragment ----- 8< -----
```

Paste everything between the markers into a terminal **on the machine being added**. It prints a short summary, and its last line is a `user@address`: send that one line back to the operator. That is your part, complete.

What the fragment does, and nothing beyond it: creates `~/.ssh` with mode 700 if it is missing, creates `~/.ssh/authorized_keys` at mode 600, appends one public key line there if that exact line is not present already — the summary says `installed` or `already present`, so running it twice is safe — checks whether an SSH server is answering on port 22 here and prints the exact setting to switch on when nothing is, and works out the address the fleet should dial. It installs no software, starts no service, fetches nothing, needs no `curl` and no network, and asks for no administrator rights.

Everything after that happens on the operator's side: they run `stado fleet enroll <name> --ssh <the address you sent> --bootstrap`, which opens the channel your paste just authorized, reads this machine's hostname and platform over it, registers it, and installs Stado here. If that install fails they get their registry entry rolled back, not a half-registered machine.

### The fragment is not a secret

It says so in its own text, and the reason is worth understanding rather than trusting. The direction is fixed: the fleet dials in to your machine, so what your machine receives is the **public** half of a key pair the operator's credential store minted, and nothing else. The private half stays in that store; it is never printed, never transmitted, and never present here. Your machine generates no pair, and no method of adding a machine to a Stado fleet sends a private key anywhere.

So a fragment somebody else reads gains them nothing — a public key is publishable by definition. Treat it as an instruction, not a credential, and send it by whatever channel actually reaches you. You can confirm exactly what was added:

```bash
tail -n 1 ~/.ssh/authorized_keys
```

One `ssh-ed25519` line, exactly the one the fragment carried. If you want to check it against the operator's own record, ask them for the fingerprint their `stado fleet invite` printed when it minted the key — the fragment quotes no fingerprint of its own, because a fingerprint computed from the same text it just installed would prove nothing.

## Let the fleet in: Remote Login

The channel is SSH and key-only — Stado never asks for your password and never uses one — so an SSH server has to be answering on this machine. Enabling it needs administrator rights, so the fragment will not do it for you; it checks, and tells you if nothing is listening.

- **macOS**: **System Settings › General › Sharing › Remote Login**, switched on, with your own user allowed under the (i) button. From a terminal: `sudo systemsetup -setremotelogin on`.
- **Linux**: install and start an SSH server (`sudo apt install openssh-server && sudo systemctl enable --now ssh`, or `openssh-server` plus `systemctl enable --now sshd` on Fedora/RHEL/Arch), and let port 22 through the host firewall.

You can turn it on before or after the paste; if you turn it on afterwards, just tell the operator it is on now — nothing here has to be redone. Until it is on, the operator's enrollment cannot reach this machine, and it will say so rather than register a machine it never read.

## The address you send back

The fragment picks the best address it can prove and prints which kind it chose: a tailnet name if this machine is on a tailnet, otherwise a `.local` multicast-DNS name where something actually answers for it, otherwise the IPv4 address of the default interface, otherwise the bare hostname. It prints that as `you@address` on its last line, and sending that line is the whole handoff.

If the operator reaches this machine at a different address, tell them — they pass the destination when they enroll, and you do not run anything again. A `.local` address is a complete way to attach and watch this machine, and it limits the operator to working from inside your network; the health reporting is unaffected either way, because it always travels outward from here.

## The one line, when the fleet has a reachable control point

Stado also has a one-line form of the same invitation, and it is worth knowing why you probably did not get one. It looks like this, with the fleet's own control address and a single-use code:

```bash
curl -fsSL <control-address>/join.sh | sh -s -- <invitation-code>
```

That line only works if this machine can reach that control address over HTTPS from where it sits — which requires the fleet to have published one: a name that resolves in DNS, an ingress in front of the control plane, and a release behind it that serves the enrollment routes. A fleet whose control plane is bound to its own loopback interface, which is the ordinary case, publishes none of that, and there is no address built into Stado that would work instead. The operator's `stado fleet invite` checks this before it prints anything: when the control point does not answer, it prints no `curl` line at all and gives them the fragment above instead. So if you received a fragment rather than a code, that check is why, and nothing is wrong.

If you did receive a code, the line above is your whole part: it installs the fleet's **public** key into your `~/.ssh/authorized_keys`, reports this machine to the fleet as a pending request, and stops. The operator then sees the request under `stado fleet pending` and finishes it with `stado fleet approve <your-hostname>`, which is also what installs Stado here — the line deliberately installs no software. Unlike the fragment, the code *is* a secret: it is single-use, expires (24 hours by default), travels as a bearer token over HTTPS, and is never written to a file or printed back to you. What the line needs present: a POSIX shell, `curl`, and permission to write `~/.ssh`. It does **not** need Stado, root, `ssh-keygen`, or a key of your own.

## If you would rather not paste anything

One other route exists, and it is the operator's to drive: **they log in once and install the key themselves.** If they can already open an SSH session here — your password, a key of theirs you already trust, or an agent — they run `stado fleet enroll <name> --ssh you@host --install-key` and no fragment and no code is involved at all. All you do is enable Remote Login and tell them how to reach you.

Stado's own footprint on your machine, once the operator enrolls it, is a per-user agent and health beacon — a launchd or systemd-user service under your account, with state under `~/.stado`. It does not require administrator rights, install system-wide services, or supply your workload's runtimes.

## Confirming your machine reports

Ask the operator for the output of:

```bash
stado registry beacon-age
```

Your machine should have a row with a fresh timestamp. A machine that never reported is listed too, with no beacon, so silence is visible rather than absent. `stado host health <name>` gives the same machine's disk, services, and log tail.

On your machine, `stado registry self --name-only` prints which registry target it is — the answer to "is this box actually the one they registered".

## Leaving

Ask the operator to remove the registry entry, then, if you also installed Stado locally, follow [Uninstall and local reset](onboarding.md#uninstall-and-local-reset). Removing the fleet's public key from `~/.ssh/authorized_keys` closes the channel from your side at any time.
