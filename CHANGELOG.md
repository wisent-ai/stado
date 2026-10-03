# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.22.18 – 0.23.7](changelog/0.22.18-0.23.7.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- `stado release policy-target-remove <product> --target <host>` stops releasing a product to one host, and `stado release policy-remove <product>` stops rolling it out by release control anywhere; each is one verified registry write, and the last target is refused with the second command named. The release agent on a host then retires what release control left there for a product whose policy no longer names the host: the release proxy its state file owns (by state path, through a new `retire` proxy-control action, since the policy no longer states the bind), the release processes it launched (only processes running out of `~/.stado/services/<product>` with the agent's launch marker), and, once nothing of it runs, its state and proxy files (`[release-agent] <product> is no longer released to this host: …`). Before, no verb removed a target, and nothing would have stopped what the agent had started there.
- `stado product install|update|status|rollback|remove <product> --surface service --host <host>` runs on `<host>` when that is another registry host: the same arguments, `--host` in its canonical name, go to the host's own Stado over its host channel and its output is printed. Since 0.23.6 the command refused (`a service installation puts its files on the machine that runs this command … run the installation on <host>`), and nothing in Stado could run it there, so a vault owner whose Skarbiec predates the `stado:role:` tag namespace could not be moved to one that has it: every release that declares a publisher, Skarbiec's own included, is refused by that Skarbiec. `--catalog` with another host is refused, because it names a file on this machine.
- `stado product install|update` clones a missing canonical checkout instead of refusing with `no canonical checkout answers for <repository> in <workspace>; no checkout was created`: the product's own repository, and every repository a Cargo or Swift git dependency of it resolves to, is cloned from its GitHub origin onto `main` at `<workspace>/<name>`, and the clone is printed. A host a service is installed on through `--host` holds no workspace checkouts of its own, so the forwarded installation stopped there. A directory already at that path that is not the checkout is refused and left alone; `sync` and `status` still create nothing. Without `WISENT_OUTPUT_DIR`, product evidence goes to `$WISENT_WORKSPACE/stado/.wisent-output` only when that is a Stado checkout and to `~/.stado/products/output` otherwise: created in the workspace, it occupied `<workspace>/stado` and every product with a `wisent-ai/stado` git dependency was refused there.
