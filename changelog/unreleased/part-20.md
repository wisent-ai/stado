# Changelog Unreleased (part 20)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **Pictures and web searches are called through Stado, as model requests are:** a product that
  needed a picture or a search had no family to hand it to, so it named a vendor or did without.
  `stado capabilities call image --consumer C` takes `{"plan": EDIT_PLAN, "inputs": [PATH…],
  "output": PATH}`, an Obraz edit plan (`obraz operations` lists its steps; `generate` makes a
  picture from a description), and Obraz runs it with `obraz edit --plan - --json
  --allow-provider-cost`; the answer is `obraz edit --json`'s (`output`, `mime_type`, `width`,
  `height`, `bytes`, `steps`). An Obraz usage refusal is the request's fault and refuses the call;
  any other Obraz failure passes to the next alternative. `stado capabilities call search
  --consumer C` takes `{"query": TEXT, "count": N}` (1 to 100) and answers `{"results": [{"title",
  "url", "snippet"}…]}`; its `search-endpoint` alternative is the address
  `capabilities.search.endpoint.url` declares (`https://` only), sent the query as `POST` JSON with
  the `token` of the item playing `capabilities.search.endpoint.key_role` when one is declared,
  and proven by any HTTP answer from it. In both families `attestation` answers a request no
  alternative before it answered: the request is kept and refused naming the command that answers
  it, and `stado capabilities attestation record image|search` takes the answer in the family's own
  shape, refused when it is not that shape. An answer an alternative gives in another shape is
  that alternative unable to answer, and the next one is asked. The orders are
  `capabilities.image.order` and `capabilities.search.order`.
- **A recipe role's kept keychain is on the user's search list only while Stado signs with it:**
  keeping a role's identity put `$HOME/.stado/signing/<name>.keychain-db` first on the search list
  and left it there. After a restart the keychain stayed listed but locked, and a product's own
  build script asking the list for a signing identity found the fleet's key there, so macOS showed
  "codesign wants to use the <name> keychain" to the operator. Keeping now takes the keychain off
  the list (an entry an earlier release left is removed on the next installation); a signature
  lists it and unlocks it just before `codesign`, and the scope's close takes it off again.
- **Every reader of the product catalog reads the committed one:** `stado product catalog`,
  `status`, `dependencies`, `advisories`, `build-trees`, `documentation`, `registry check-claims`
  and Stado's `change` and `quality hooks` read a catalog its checkout tracks at the committed
  head (or `origin/main` when the head is behind), as installations already did. One session's
  uncommitted edit of the shared checkout that does not validate no longer refuses every other
  session's read: it is named on stderr with its error and not read. `catalog --json` names the
  revision it read as `catalog_read`, and `registry add|set|rm` say under `uncommitted` that
  readers see the edit once it is committed, with the command that commits it. A `--catalog`
  file no checkout tracks is read as it stands.
