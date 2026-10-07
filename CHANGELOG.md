# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.43](changelog/0.23.42-0.23.43.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado release catalog retire <product> [--target HOST]… [--json]`, the inverse of `enroll` (cf966ef9):** the release catalog had verbs that add a product (`enroll`, `sync`) and none that removes one, while `withdraw-publisher` refuses as long as the catalog holds the product, so retiring Skarbiec Hub needed `stado storage rm stado://system/release-catalog/skarbiec-hub.json` by hand. `retire` removes the product's catalog entry, so the daily batch stops building it, and then withdraws its publisher exactly as `withdraw-publisher` does; published releases stay and a returning product enrolls again. A product the catalog does not hold is refused by name, naming `catalog audit`; `withdraw-publisher`'s refusal for a catalogued product now names `retire`. `stado storage rm` and `retire` delete through one function, which refuses a release object.

- **The release and object verifier reconcile reads the owner vault with the installed Skarbiec (960e3bb9):** `stado repair stado --step release-verifier`, which `catalog declare-publisher` and `withdraw-publisher` run on the vault owner, listed the owner vault through `~/.stado/bin/skarbiec-keychain-launcher`, a script nothing installs, so on charless-mac-mini every publisher change ended `no installed Skarbiec launcher at $HOME/.stado/bin/skarbiec-keychain-launcher` with the declarations changed and the verifier's grants not. The reconcile now lists the owner vault the way every other owner-path call does: the installed `skarbiec` against the resolved owner vault.

- **`app-check --command-array FILE:ARRAY` reads a CLI's commands from the array its usage text is built from (da52b19e):** `--usage-commands` reads the names under `commands:` in the `USAGE` literal, so a CLI that builds that text from a command array (Glina's `COMMANDS` in `pipeline/arguments.js`) answered `commands: lists no command`. Every `name:` string inside the named array is a `cmd:` name, read through the same masking pass as `--mcp-tools`, which now shares that reader; an argument that is not `FILE:ARRAY` and an array naming no command are refused by name.

- **A Skarbiec refusal on the vault owner is stated as refused, not as an outage (7353f650):** `stado credentials token mint`, `grant` and every other command that runs the owner's Skarbiec reported any answer Skarbiec gave as `infra_down` (exit 69, "retry later"), so `grant issue requires --ttl-seconds` from an owner whose Skarbiec predates grants that live until revoked read as a network problem to wait out. The host channel reaching the host and Skarbiec answering is now `refused`, printed as `<host>: Skarbiec <command> refused: <Skarbiec's line>`; a request for a grant until revoked adds which Skarbiec executable on which host answered, the one to bring to the release that implements that lifetime. A host channel that does not reach the host is still `infra_down`.
