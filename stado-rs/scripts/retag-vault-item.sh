#!/bin/sh
# Replace the tags of one Skarbiec item on this host, and prove what changed.
#
# Tags are not decoration on a vault item: consumers enumerate by them. Brama
# treats an item as a spendable subscription only when it carries
# `brama:subscription` and `brama:agent:<agent>`, so an item that loses those
# tags disappears from the fleet while its credential stays valid and every
# health check keeps reporting green. That is not a hypothetical -- it took a
# working Kimi subscription out of service for a day, and the vault item was at
# revision 144 with zero tags while `/readyz` still answered `ready: true`.
#
# A retag is an owner write, so it can only run where the owner key is: on the
# host itself, against $HOME/.stado/skarbiec.vault.json. It replaces tags only
# and never touches or re-encrypts the payload, which is exactly why this
# exists as its own operation rather than as a `set-json` that would rewrite a
# live credential to restore a label.
#
# The caller prepends `item` and `tags` as shell-quoted bindings. Reports
# tab-delimited STADO_RETAG markers -- before, after -- so the caller states
# what the host had and has rather than asserting success.
set -eu
PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
export PATH
GNUPGHOME="${GNUPGHOME:-$HOME/.gnupg}"
export GNUPGHOME
SKARBIEC="$HOME/.stado/bin/skarbiec"
SKARBIEC_VAULT_FILE="${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}"
export SKARBIEC_VAULT_FILE

if [ ! -x "$SKARBIEC" ]; then
  printf 'no Skarbiec binary at %s\n' "$SKARBIEC" > /dev/stderr
  exit 1
fi
if [ ! -f "$SKARBIEC_VAULT_FILE" ]; then
  printf 'no vault at %s\n' "$SKARBIEC_VAULT_FILE" > /dev/stderr
  exit 1
fi

# Whether this build can retag at all. The discriminator is the usage literal,
# never the bare command name: rustc packs string literals into one
# unterminated blob, so a binary that carries the command shows
# `...setgetretagdelete...` on a single line and a whole-line match for `retag`
# reports absent on a build that has it. That false negative cost an hour and
# sent one diagnosis at the wrong host.
if ! strings -a "$SKARBIEC" 2>/dev/null | grep -q 'usage: retag <id> --tags'; then
  printf 'the Skarbiec build at %s predates the retag operation\n' "$SKARBIEC" > /dev/stderr
  exit 1
fi

report() {
  python3 - "$SKARBIEC_VAULT_FILE" "$item" "$1" <<'PY'
import json, sys
vault_path, item_id, phase = sys.argv[1], sys.argv[2], sys.argv[3]
item = json.load(open(vault_path)).get("items", {}).get(item_id)
if item is None:
    print(f"STADO_RETAG\t{phase}\tabsent\t-\t-")
else:
    tags = item.get("tags") or []
    print(
        "STADO_RETAG\t{phase}\t{state}\t{revision}\t{tags}".format(
            phase=phase,
            state=item.get("state") or "-",
            revision=item.get("revision") if item.get("revision") is not None else "-",
            tags=",".join(tags) if tags else "-",
        )
    )
PY
}

report before
"$SKARBIEC" retag "$item" --tags "$tags" > /dev/null
report after
