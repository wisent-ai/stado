#!/bin/sh
# Build the stado binary for linux-amd64 from a macOS workstation.
#
#   sh stado-rs/scripts/cross-build-linux-amd64.sh
#
# Prints the artifact path and its SHA-256 on success. Install it with
# `stado host install-binary <target> --from <path>`, which records the
# producing commit in the target's provenance manifest.
#
# Why this file exists. `.wisent-release.json` declares a linux-amd64 platform
# whose recipe expects a linux runner, and this fleet has no linux build
# consumer, so that platform has never been built through the release pipeline.
# Meanwhile ubuntu-server-rtx-pro-6000 has been running a linux stado all along.
# `stado host provenance` showed it carrying an artifact whose producer was
# nowhere in this repository, and there was no cross configuration checked in
# anywhere -- so the capability existed only in somebody's shell history. This
# is that capability, written down, so the next linux binary on the fleet can
# name the commit and the recipe that made it.
#
# It is deliberately NOT wired into the release manifest. A cross build on a
# workstation is not the same evidence as a build on the target platform, and
# quietly substituting one for the other is the kind of undeclared shortcut this
# repository has spent a week removing. Use it to deliver a binary today; fix
# the pipeline by giving the fleet a linux build consumer.
set -eu

target=x86_64-unknown-linux-gnu
# Old enough to run on every Linux in this fleet. zig links against the glibc
# version named here rather than the build host's, which is the entire reason a
# macOS workstation can produce a binary Ubuntu will load.
glibc=x86_64-linux-gnu.2.31

command -v zig >/dev/null 2>&1 || {
  printf 'zig is required: brew install zig\n' >&2
  exit 69
}
command -v cargo >/dev/null 2>&1 || {
  printf 'cargo is required\n' >&2
  exit 69
}
rustup target list --installed 2>/dev/null | grep -qx "$target" || {
  printf 'rust target %s is not installed: rustup target add %s\n' "$target" "$target" >&2
  exit 69
}

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
shim=$(mktemp -d)
trap 'rm -rf "$shim"' EXIT INT TERM

# cc-rs appends `--target=<rust triple>` to every invocation, and zig rejects
# that spelling with UnknownOperatingSystem. Dropping it is what makes ring and
# aws-lc-sys compile; zig already has its target from the -target flag below.
cat > "$shim/zig-cc" <<SHIM
#!/bin/sh
args=""
for a in "\$@"; do
  case "\$a" in
    --target=*) continue ;;
  esac
  args="\$args \"\$a\""
done
eval exec zig cc -target $glibc "\$args"
SHIM
cat > "$shim/zig-ar" <<'SHIM'
#!/bin/sh
exec zig ar "$@"
SHIM
chmod +x "$shim/zig-cc" "$shim/zig-ar"

CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="$shim/zig-cc" \
CC_x86_64_unknown_linux_gnu="$shim/zig-cc" \
AR_x86_64_unknown_linux_gnu="$shim/zig-ar" \
  cargo build \
    --manifest-path "$root/stado-rs/Cargo.toml" \
    --locked --release --target "$target" --bin stado

artifact="$root/stado-rs/target/$target/release/stado"
[ -f "$artifact" ] || {
  printf 'cargo reported success but %s is absent\n' "$artifact" >&2
  exit 70
}

# Refuse to hand back a Mach-O binary that happens to sit at the target path,
# which is what a misconfigured linker produces and what nobody notices until
# the install fails on the far side.
case "$(od -An -tx1 -N4 "$artifact" | tr -d ' ')" in
  7f454c46) : ;;
  *) printf 'not an ELF binary: %s\n' "$artifact" >&2; exit 70 ;;
esac

printf '%s\n' "$artifact"
shasum -a 256 "$artifact" | awk '{print $1}'
