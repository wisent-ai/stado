#!/bin/sh
# Which variables does this host's Brama service env file actually define?
#
# `com.wisent.always-on.brama` reports failed and only its entitlements router
# survives a restart; the wrapper `start-with-skarbiec` exits early when a
# required variable is absent, and its last recorded complaint was
# BRAMA_GNUPG_HOME. Names and set/unset only: no value is printed, and the
# file is never modified.
set -eu

env_file=${BRAMA_SERVICE_ENV_FILE:-$HOME/.config/brama/service.env}

if [ ! -r "$env_file" ]; then
    printf 'no readable service env at %s\n' "$env_file"
else
    printf 'service env %s\n' "$env_file"
    /usr/bin/sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/defined \1/p' "$env_file" | /usr/bin/sort

    for name in BRAMA_GNUPG_HOME BRAMA_PORT BRAMA_UPSTREAM SKARBIEC_VAULT_FILE; do
        if /usr/bin/grep -q "^$name=" "$env_file"; then
            printf 'required %-20s present\n' "$name"
        else
            printf 'required %-20s MISSING\n' "$name"
        fi
    done

    gnupg=$(/usr/bin/sed -n 's/^BRAMA_GNUPG_HOME=//p' "$env_file")
    if [ -n "${gnupg:-}" ] && [ -d "$gnupg" ]; then
        printf 'gnupg home %s exists\n' "$gnupg"
    elif [ -n "${gnupg:-}" ]; then
        printf 'gnupg home %s is declared but absent\n' "$gnupg"
    else
        printf 'gnupg home not declared; default candidate %s\n' "$HOME/.gnupg"
    fi

    routes=$(/usr/bin/sed -n 's/^BRAMA_INFERENCE_ROUTES_FILE=//p' "$env_file")
    if [ -n "${routes:-}" ] && [ -r "$routes" ]; then
        printf 'inference routes %s\n' "$routes"
        /bin/cat "$routes"
        printf '\n'
    elif [ -n "${routes:-}" ]; then
        printf 'inference routes %s are unreadable\n' "$routes"
    else
        printf 'inference routes are not declared\n'
    fi
fi
