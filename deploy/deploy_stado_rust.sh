#!/bin/bash
# Provider-neutral Rust control-plane deploy.
#
# Release publication is owned by the Azure GitHub workflow. This script runs
# on the coordinator host, installs an optional already-built release artifact,
# then delegates all persistent service rendering to `stado bootstrap --local`.
# It never provisions cloud resources and never consults gcloud, gsutil, ADC,
# Python deployment code, or a Cloud Run image.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# One managed path, and it is the product's, not this script's. `release
# install-local` installs into `$HOME/.stado/bin` by construction, every
# rendered unit runs from there, and `service converge` attests against it, so a
# `STADO_INSTALL_DIR` override could only have pointed this script somewhere the
# rest of the fleet does not look. Nothing in this repository set it.
INSTALL_DIR="$HOME/.stado/bin"
RELEASE_DIR="${STADO_RELEASE_DIR:-}"

# The platform is resolved before anything is installed, because the delivery
# below names the archive and the manifest by it. Same mapping the bootstrap
# installer and every host helper use, so all of them name one release the same
# way.
if [ -z "${STADO_RELEASE_PLATFORM:-}" ]; then
    case "$(uname -s)-$(uname -m)" in
        Darwin-arm64) STADO_RELEASE_PLATFORM=darwin-arm64 ;;
        Linux-x86_64) STADO_RELEASE_PLATFORM=linux-amd64 ;;
        *)
            echo "FATAL: unsupported release platform: $(uname -s) $(uname -m)"
            false
            ;;
    esac
fi
export STADO_RELEASE_PLATFORM

# Installed through the product's own delivery endpoint, not by copying files
# into place.
#
# `stado release install-local` verifies the archive against the digest the
# platform manifest declares, installs by rename, and leaves the attestation
# copy under `$HOME/.stado/releases/<binary>/<version>/<platform>/` that
# `cli::service_converge::attest_installed` byte-compares against the installed
# file. A plain `cp` leaves no such copy, so on 2026-09-02 the 0.13.46 train
# deployed this host correctly and then `deploy-fleet` refused it: `stado
# service converge lukasz-macbook stado` reported the binary `unattested` —
# "the host runs 0.13.46 and no delivered copy of 0.13.46 is staged" — and
# refused rather than downgrade a host that was already right. The bytes were
# fine; nothing had recorded where they came from.
if [ -n "$RELEASE_DIR" ]; then
    if [ ! -x "$RELEASE_DIR/stado" ]; then
        echo "FATAL: STADO_RELEASE_DIR=$RELEASE_DIR has no executable Rust stado binary"
        false
    fi
    RELEASE_MANIFEST="$RELEASE_DIR/release-manifest-$STADO_RELEASE_PLATFORM.json"
    if [ ! -r "$RELEASE_MANIFEST" ]; then
        echo "FATAL: $RELEASE_DIR carries no readable $RELEASE_MANIFEST"
        false
    fi
    DELIVERED_VERSION="$(jq -er '.version' "$RELEASE_MANIFEST")"
    DELIVERED_SHA256="$(jq -er '.sha256' "$RELEASE_MANIFEST")"
    DELIVERED_ARCHIVE="$RELEASE_DIR/stado-v$DELIVERED_VERSION-$STADO_RELEASE_PLATFORM.tar.gz"
    if [ ! -r "$DELIVERED_ARCHIVE" ]; then
        echo "FATAL: $DELIVERED_ARCHIVE is the archive the manifest names and it is not readable"
        false
    fi
    mkdir -p "$INSTALL_DIR"
    for name in stado stado-coverage stado-fix stado-watchdog stado-mcp; do
        [ -f "$RELEASE_DIR/$name" ] || continue
        WISENT_PRODUCT=stado \
        WISENT_VERSION="$DELIVERED_VERSION" \
        WISENT_RELEASE_ARCHIVE="$DELIVERED_ARCHIVE" \
        WISENT_RELEASE_SHA256="$DELIVERED_SHA256" \
            env -u STADO_API_TOKEN "$RELEASE_DIR/stado" release install-local \
                --member "$name" --name "$name"
    done
    # Leave the receipt `stado service converge` reads before the bytes are
    # anything but a version string. Never fatal, for the reason
    # `self_update::install_release_with` gives at the same point: the bytes are
    # already verified against the canonical manifest and the install is the
    # point, so a receipt that cannot be written is reported and the delivery
    # continues — visibly, because a silent miss is what made every delivery to
    # this host read `unattested`.
    if ! "$SCRIPT_DIR/stage_release_attestation.sh" "$RELEASE_DIR"; then
        echo "WARNING: installed from $RELEASE_DIR but staged no attestation copy," \
            "so 'stado service converge' will read these bytes as unattested"
    fi
fi

STADO_BIN="${STADO_BIN:-$INSTALL_DIR/stado}"
if [ ! -x "$STADO_BIN" ]; then
    echo "FATAL: Rust stado binary unresolved; set STADO_BIN or STADO_RELEASE_DIR"
    false
fi
export STADO_BIN

# The release gate downstream asks whether the public channel serves the exact
# release this host runs. Until 2026-09-02 it asked about `release.version` in
# the operator config, which no deploy ever writes: the 0.13.42 deploy verified
# 0.7.22, a version published in July. The coordinate that cannot drift is the
# binary that was just installed, so it names itself here unless the caller
# pinned one on purpose.
if [ -z "${STADO_RELEASE_VERSION:-}" ]; then
    INSTALLED_VERSION="$("$STADO_BIN" --version)"
    INSTALLED_VERSION="${INSTALLED_VERSION##* }"
    case "$INSTALLED_VERSION" in
        *[![:alnum:]._-]*|"")
            echo "FATAL: $STADO_BIN --version did not name a usable release version"
            false
            ;;
    esac
    STADO_RELEASE_VERSION="$INSTALLED_VERSION"
fi
export STADO_RELEASE_VERSION

# A deploy that installs a release and leaves the profile declaring an older one
# is the drift this whole section exists for: `release.version` is what
# `stado agent` dispatch, `local_install` and the version-drift check all read as
# "the release this deployment wants". It is written through Stado's own config
# command, and only when it disagrees, so the file is untouched on a no-op run.
# `config show` resolves env over file, and this script has just exported the
# version, so the file's own declaration is only visible with that variable
# unset — otherwise the comparison always agrees with itself and never writes.
# The profile is the installer's own precondition and it refuses without one a
# few lines later with a sentence that says so; this write stays quiet until
# there is a file to write, rather than failing the deploy with a confusing
# config error before that refusal is reached.
if [ -n "${STADO_CONFIG:-}" ] && [ -r "${STADO_CONFIG:-}" ]; then
    DECLARED_VERSION="$(env -u STADO_RELEASE_VERSION "$STADO_BIN" config show 2>/dev/null |
        jq -r '.resolved.stado_release_version // ""' 2>/dev/null || true)"
    if [ "$DECLARED_VERSION" != "$STADO_RELEASE_VERSION" ]; then
        "$STADO_BIN" config set release.version "$STADO_RELEASE_VERSION"
        echo "declared release.version=$STADO_RELEASE_VERSION (was ${DECLARED_VERSION:-unset})"
    fi
fi

echo "Deploying Rust Stado from the explicitly selected deployment profile."
echo "Preflight fails closed on unresolved active storage, replica, identity,"
echo "release, object auth, networking or quota; fenced providers are never contacted."
exec "$SCRIPT_DIR/install_macos_coordinator.sh"
