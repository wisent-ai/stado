# Changelog

Entries for the release being prepared live here. Released entries move into
`changelog/`, one file per version range, so the active file remains within
the repository's file-size limit.

The version-bump commit moves them with `stado product changelog --version V`;
`stado build submit` refuses a revision whose Unreleased section still holds entries.

## Released

- [0.23.12 – 0.23.30](changelog/0.23.12-0.23.30.md)
- [0.22.18 – 0.23.11](changelog/0.22.18-0.23.11.md)
- [0.22.17 – 0.22.18](changelog/0.22.17-0.22.18.md)
- [0.16.41 – 0.22.15](changelog/0.16.41-0.22.15.md)
- [0.16.20 – 0.16.40](changelog/0.16.20-0.16.40.md)
- [0.16.1 – 0.16.19](changelog/0.16.1-0.16.19.md)
- [0.15](changelog/0.15.md)

## Unreleased

- **`stado host compiler-cache TARGET status|ensure|remove` (f8c5a78e):** a builder whose worker cannot install the compiler cache refuses every release build, the one that would repair its worker included, and nothing placed through the queue reaches it. This runs TARGET's own `stado product compiler-cache` over the fleet channel, with `~/.cargo/bin` ahead of the channel's PATH so a Stado that still runs a bare `cargo` installs it too, and prints the host's own report; a refusal names the state the host reports (`refused: the host reports absent`).
