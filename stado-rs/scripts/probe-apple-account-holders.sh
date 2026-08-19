#!/bin/sh
# Which local users on this host are signed into an Apple account, and where the
# answer could not be obtained.
#
# `stado identity verify` reads `defaults read MobileMeAccounts`, which answers only
# for the user the registry channel logs in as. A binding naming any other user is
# therefore reported `unknown` forever, and nobody can tell "that user is not signed
# in" from "this probe was not allowed to look" -- two states that call for opposite
# actions from an operator.
#
# So this reads the preference file directly, per user, and reports one of three
# words for each: the account identifiers it found, `unreadable` when the file exists
# but this process may not open it, or `none` when there is no such file. Only the
# first is an observation; the other two are the probe admitting its limit, which is
# the distinction the whole thing exists to make.
#
# This script is embedded in the stado binary itself
# (`identity::APPLE_ACCOUNT_PROBE`, via include_str!) and run by
# `stado identity verify` as one fixed remote script -- nothing is installed on
# the host and nothing is left behind.
#
# Read-only throughout: it opens preference files and writes nothing anywhere.
set -eu

for home in /Users/*; do
  user=$(basename "$home")
  case $user in
    Shared|Guest|.*) continue ;;
  esac
  plist=$home/Library/Preferences/MobileMeAccounts.plist
  if [ ! -f "$plist" ]; then
    printf '%s\tnone\n' "$user"
    continue
  fi
  if [ ! -r "$plist" ]; then
    printf '%s\tunreadable\n' "$user"
    continue
  fi
  accounts=$(/usr/bin/plutil -p "$plist" | /usr/bin/awk -F'"' '/AccountID/ { print $4 }' | /usr/bin/tr '\n' ' ')
  accounts=$(printf '%s' "$accounts" | /usr/bin/sed -e 's/ *$//')
  if [ -z "$accounts" ]; then
    printf '%s\tnone\n' "$user"
  else
    printf '%s\t%s\n' "$user" "$accounts"
  fi
done
