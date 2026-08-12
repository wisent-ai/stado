#!/bin/sh
# Report the isolated Linux pre-check runner without exposing its credentials.
# Run through `stado host install-helper` + `run-helper`.
set -u

unit=wisent-stado-precheck-runner.service

printf '%s\n' '== systemd =='
systemctl status "$unit" --no-pager || true
printf '%s\n' '== recent journal =='
journalctl --unit "$unit" --lines 100 --no-pager || true
printf '%s\n' '== network boundary =='
nft list table inet stado_precheck || true
