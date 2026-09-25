#!/usr/bin/env bash
# Run by the release worker after its build; never schedules another release.
set -euo pipefail
source_dir=${WISENT_SOURCE_DIR:?release source is required}
output_dir=${WISENT_OUTPUT_DIR:?retained release output is required}
export STADO_BIN="$output_dir/stado-rs/target/release/stado"
export STADO_EXPANSION_EVIDENCE_DIR="${WISENT_TEST_EVIDENCE_DIR:?retained test evidence directory is required}/fleet-expansion"
mkdir -p "$STADO_EXPANSION_EVIDENCE_DIR"
test -x "$STADO_BIN" || { printf 'built Stado is missing: %s\n' "$STADO_BIN" >&2; exit 66; }
case "${1:-}" in
  cli)
    cargo test --manifest-path "$source_dir/stado-rs/Cargo.toml" --locked --release --test fleet_expansion -- --nocapture
    ;;
  desktop)
    # The job's checkout is not a canonical workspace, and `stado product
    # swift` resolves dependencies from the operator's canonical checkouts:
    # on a release worker that read ~/Documents/CodingProjects/Wisent and was
    # refused by macOS (`reading workspace …: Operation not permitted`).
    # The package pins its dependencies to published tags, so SwiftPM builds
    # it from this source alone, in the package's own `.build` inside the
    # job tree. `FleetTests` holds the screens that drive STADO_BIN through a
    # real isolated API: Fleet Expansion, Products and the build operations.
    swift test --package-path "$source_dir/desktop/StadoDesktop" --filter FleetTests
    ;;
  *) printf 'usage: qualify.sh cli|desktop\n' >&2; exit 64 ;;
esac
