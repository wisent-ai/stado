# Changelog Unreleased (part 5)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- `stado_database::sync::Client` no longer panics with Tokio's `Cannot drop a runtime in a
  context where blocking is not allowed` when it is dropped, or refused while connecting, inside
  a product's own async runtime. A product that held it there answered an unreachable database
  with that panic and exit 101 instead of the refusal naming the step and Stado's answer, unless
  it opened and released the client on threads of its own. The client
  now releases its runtime by leaving a multi-threaded worker for the wait, shutting it down
  without waiting on a current-thread runtime, and dropping it anywhere else.
