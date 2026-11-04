# Changelog Unreleased (part 21)

Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.
The version bump gathers them into the release's own file and removes this one.

## Unreleased

### Fixed

- **A unit launchd runs is no longer started a second time by an old cron fallback:** when every
  launchd domain refused an install made over SSH, the install fell back to a crontab line
  `@reboot /bin/sh ~/.stado/bin/run-<unit>.sh`, and a later install that launchd accepted left
  that line in place. Every boot then started the unit twice; the copy that lost the race for its
  port restarted for ever, and its published state said the unit was down while the other copy
  served. Every install that launchd accepts, in the per-login domains and the system domain,
  now removes the line and its wrapper and says `removed the @reboot cron fallback of <unit>;
  launchd alone starts it`. A crontab it cannot rewrite is refused naming the unit and the wrapper
  the line still names.
