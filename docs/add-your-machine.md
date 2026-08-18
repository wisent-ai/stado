# Add your own machine

For the person whose computer is being added to a fleet somebody else runs. Normally your whole part is one line. If you are the operator, use [Onboard another machine](onboarding.md#onboard-another-machine) instead — that page covers all four methods and this one deliberately repeats none of it.

## The one line

The operator sends you an invitation code. Run this on the machine being added, with the code they sent you in place of `<invitation-code>`:

```bash
curl -fsSL https://stado.wisent.com/join.sh | sh -s -- <invitation-code>
```

That is your part. The script installs the fleet's **public** key into your `~/.ssh/authorized_keys`, reports this machine to the fleet as a pending request, and stops. Everything after that happens on the operator's side: they see the request under `stado fleet pending` and finish it with `stado fleet approve <your-hostname>`, which is also what installs Stado here — the line above deliberately installs no software.

The code is single-use and expires (24 hours by default). It travels as a bearer token over HTTPS, is never printed back to you, never written to a file on this machine, and never appears in this machine's process list. If it is mistyped, the script says so before contacting anyone.

What the line needs present: a POSIX shell, `curl`, and permission to write `~/.ssh`. It does **not** need Stado, root, `ssh-keygen`, or a key of your own.

### The key you receive is a public key

The direction is fixed: the fleet dials in to your machine, so your machine receives the public half of a key pair the operator's credential store minted, and nothing else. The private half stays in that store; it is never printed, never transmitted, and never present here. Your machine does not generate a pair, and no method of adding a machine to a Stado fleet sends a private key anywhere. You can confirm exactly what was added:

```bash
tail -n 1 ~/.ssh/authorized_keys
```

One `ssh-ed25519` line, matching the SHA256 fingerprint the script printed. Running the line twice does not add the key twice. The fingerprint comes from `ssh-keygen` or `openssl`, whichever is present; on a machine with neither it is reported empty and the operator verifies the key when they approve.

## Let the fleet in: Remote Login

The channel is SSH and key-only — Stado never asks for your password and never uses one — so an SSH server has to be answering on this machine. Enabling it needs administrator rights, so the script will not do it for you; it checks and tells you if nothing is listening, and reports that state to the operator so they do not waste an approval on an unreachable machine.

- **macOS**: **System Settings › General › Sharing › Remote Login**, switched on, with your own user allowed under the (i) button. From a terminal: `sudo systemsetup -setremotelogin on`.
- **Linux**: install and start an SSH server (`sudo apt install openssh-server && sudo systemctl enable --now ssh`, or `openssh-server` plus `systemctl enable --now sshd` on Fedora/RHEL/Arch), and let port 22 through the host firewall.

You can turn it on before or after running the line; if you turn it on afterwards, just tell the operator it is on now — nothing here has to be redone.

## The address it reported

The script picks the best address it can prove and prints which kind it chose: a tailnet name if this machine is on a tailnet, otherwise a `.local` multicast-DNS name where something actually answers for it, otherwise the IPv4 address of the default interface, otherwise the bare hostname. It reports that as `you@address`.

If the operator reaches this machine at a different address, tell them — they set the final destination when approving, and you do not run anything again. A `.local` address is a complete way to attach and watch this machine, and it limits the operator to working from inside your network; the health reporting is unaffected either way, because it always travels outward from here.

## If you cannot run that line

Two other routes exist, and both are the operator's to drive:

- **They log in once and install the key themselves.** If they can already open an SSH session here — your password, a key of theirs you already trust, or an agent — they run `stado fleet enroll <name> --ssh you@host --install-key` and no code is involved at all. All you do is enable Remote Login and tell them how to reach you.
- **You paste the key by hand.** For a machine with no outward HTTPS, or when you would rather not run a fetched script, ask the operator for the public-key line from `stado fleet key generate <name>` and append exactly that line yourself:

  ```bash
  mkdir -p ~/.ssh && chmod 700 ~/.ssh
  printf '%s\n' '<the line the operator sent you>' >> ~/.ssh/authorized_keys
  chmod 600 ~/.ssh/authorized_keys
  ```

  Then give them a destination they can actually open, in `user@host` form, and they run the same probing enrollment. This is the older, slower path: it needs a person on each end at the same time, which is precisely what the invitation code removes.

Stado's own footprint on your machine, once the operator approves, is a per-user agent and health beacon — a launchd or systemd-user service under your account, with state under `~/.stado`. It does not require administrator rights, install system-wide services, or supply your workload's runtimes.

## Confirming your machine reports

Ask the operator for the output of:

```bash
stado registry beacon-age
```

Your machine should have a row with a fresh timestamp. A machine that never reported is listed too, with no beacon, so silence is visible rather than absent. `stado host health <name>` gives the same machine's disk, services, and log tail.

On your machine, `stado registry self --name-only` prints which registry target it is — the answer to "is this box actually the one they registered".

## Leaving

Ask the operator to remove the registry entry, then, if you also installed Stado locally, follow [Uninstall and local reset](onboarding.md#uninstall-and-local-reset). Removing the fleet's public key from `~/.ssh/authorized_keys` closes the channel from your side at any time.
