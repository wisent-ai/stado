# Changelog Unreleased (part 14)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Added

- **What one product calls another for has alternatives, and Stado hands out the one that
  answers:** a product that needed a model, a browser, a secret, a mailbox or a transcript dialled
  the product that provides it, so Oko and Weles stopped whole when Brama did not answer. The
  capability catalog now declares the families products resolve through Stado (`inference`:
  Brama, then the OpenAI-compatible endpoint `capabilities.inference.endpoint.url` names, asked
  for `capabilities.inference.endpoint.model` with the key of the item playing role
  `capabilities.inference.endpoint.key_role`; `secrets`: Skarbiec, then the owner-only file store;
  `browser-automation`: Weles; `mail`: Skrzynka; `transcripts`: Transcript Lake).
  `stado capabilities resolve FAMILY --consumer C [--failed P] [--json]` answers the first
  alternative in the selected order that is proven to answer, with every one before it that did
  not; when none answers it exits with the `infra_down` code and its `details.unreachable` names
  each alternative's product, route and cause. A proof stands for the directory refresh interval
  this host's resolver declares, so a down first alternative costs one proof per interval, and a
  caller refused by the alternative it was handed reports it with `--failed` to have it proven
  again at once. Every answer is kept in `~/.stado/capabilities.json` (`STADO_CAPABILITIES_RECORD`
  moves it), and `stado capabilities [FAMILY]` prints each family's order, each alternative's
  last proof and which alternative served which consumer, a fallback marked as one.
  `stado capabilities select FAMILY --order A,B` writes `capabilities.<family>.order` after
  refusing a name the family does not declare or names twice. Stado Desktop's Compute providers
  screen runs both.
- **A product that dials another product's service itself is refused before it is pushed:**
  `stado quality check`, which every product's pre-push, change hand-off, release submission and
  delivery gate run, now refuses a revision whose added lines reach a service a capability
  family offers alternatives for (Brama, Skarbiec) through `stado service directory connect
  <service>` or its forward marker, naming each line and the `stado capabilities resolve
  <family> --consumer <product>` that replaces it. Only the lines the revision adds against the
  published main are judged; prose, `tests/`, `docs/` and `.github/` are not read, and Stado,
  which serves the directory, is not judged by it.
