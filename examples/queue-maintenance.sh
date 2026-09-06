#!/bin/sh
# queue-maintenance.sh — stop dispatch, let running work finish, reopen.
# Nothing is cancelled; queued jobs wait for resume.
# Run: sh queue-maintenance.sh
set -eu

stado queue status
stado queue pause
stado queue status
stado queue drain
stado queue resume
stado queue status
