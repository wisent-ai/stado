# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.42 – 0.23.51](changelog/0.23.42-0.23.51.md)
- [0.23.12 – 0.23.41](changelog/0.23.12-0.23.41.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **A restart is not called failed because another job listens on the same port:** the wait after a service restart (`stado service restart`, `stado host config-set --reload-service`) answered `<unit> is not serving — <port> is held by <pid> <program> (<label>), so the restarted unit cannot bind it` on its first read whenever another job held the port, although a listener on another address of that port does not stop the unit from binding its own: on charless-mac-mini the tailnet object proxy listens on 8765 in front of Stado's loopback 8765, and every restart of `com.wisent.stado` there was reported failed while it came back (090574f5). The wait now ends only on what the unit does — it serves every declared port, launchd holds no process for it, or launchd started a new process — and names the other holder in those two failures (`…; its own log says why; <port> is held by …`).
