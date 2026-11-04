# kompilator

`kompilator` builds a Rust package from its `Cargo.lock` by running `rustc`
directly, without Cargo. It is written in Rust with nothing but the standard
library, so it is itself built with `rustc` alone:

```sh
rustc --edition 2021 -O src/main.rs -o .build/kompilator
```

## Build a package

```sh
kompilator build --manifest-path ../oko/rust/Cargo.toml --release --bin oko
```

A build prints the path of every binary it linked on standard output, one per
line, after checking each is there; its progress goes to standard error. A
caller takes the binary's path from that answer, or from `kompilator where`,
never by spelling the output directory itself.

| Flag | Meaning |
|---|---|
| `--manifest-path DIR/Cargo.toml` | the package to build; its `Cargo.lock` must stand beside it |
| `--release` | the release profile; the dev profile without it. The manifest's `[profile.release]` or `[profile.dev]`, and its `build-override`, apply |
| `--bin NAME` | link only this binary; every binary of the package when none is named |
| `--out DIR` | where the binaries go; `<package dir>/.build/kompilator/<release or debug>` by default |
| `--cache DIR` | where compiled units are kept; `KOMPILATOR_CACHE`, else the `--out` directory. Packages built with one cache share every unit they have in common |
| `--jobs N` | how many units compile at once; the machine's cores by default |
| `--incremental` | compile the package's own sources incrementally whatever the profile says |
| `--features A,B` | enable these features of the package; repeat the flag or separate names with commas or spaces |
| `--no-default-features` | leave the package's `default` feature off |
| `--all-features` | enable every feature the package declares |
| `--revision REV` | build this commit (or branch, tag) of the manifest's repository instead of its working tree: the committed tree is exported once under `<out>/revisions/<commit>`, so edits not yet committed never enter the build. `WISENT_SOURCE_COMMIT` is set to the commit for build scripts that stamp a revision, unless it is already set |

`RUSTC` names the compiler. Without it, the compiler is the `rustc` the
toolchain manager chooses in the package's own directory (its
`rust-toolchain.toml`, else the default toolchain), resolved once to its full
path through `rustc --print sysroot`, so the same compiler builds every unit
and runs in every build script whatever directory `kompilator` is started
from. The compiler's version is part of every unit's identity: a different
toolchain compiles every unit again.

## Find a built binary

```sh
kompilator where --manifest-path ../stado/stado-rs/Cargo.toml --release --bin stado
```

`where` takes the flags that decide where binaries are written
(`--manifest-path`, `--release`, `--bin`, `--out`) and prints, one per line,
the path each selected binary has in that build's output, without building.
A binary that is not there is refused with the build command that writes it:
``<name> has not been built: <path> does not exist; `kompilator build
--manifest-path <dir>/Cargo.toml --release --bin <name>` builds it there``.
Any other flag is refused (exit `2`), because it changes what is compiled,
not where.

## What it does

1. Reads `Cargo.lock` and every locked package's `Cargo.toml`. A registry
   package is read from the registry's unpacked sources under Cargo's home
   (`CARGO_HOME`, else `~/.cargo`); one that is not there is downloaded once
   from crates.io with `curl`, checked against the SHA-256 its lock entry
   pins, and unpacked under `<cache>/registry`. A Git package is exported at
   its locked commit into the cache from Cargo's Git database, else from the
   build's own bare repository under `<cache>/git/db`, fetched once from its
   URL with `git` and this machine's Git credentials. A path package is read
   from its path. Only the packages the lock names are fetched, at the
   versions and commits it names.
2. Resolves features the way Cargo's second resolver does: each package is
   compiled once for the target and, when a build script or a procedural
   macro needs it, once for the host, each with the features that side asks
   for. `dep:x`, implicit optional-dependency features, `x/f` and weak `x?/f`
   are honoured; `[target.'cfg(…)']` tables are judged against
   `rustc --print cfg`.
3. Compiles units in parallel. A library starts as soon as the libraries under
   it have written their interface (`.rmeta`); build scripts, procedural
   macros and binaries wait for finished libraries. Build scripts run with the
   environment Cargo documents (`OUT_DIR`, `TARGET`, `CARGO_FEATURE_*`,
   `CARGO_CFG_*`, `DEP_<LINKS>_*`, `CARGO_PKG_*` …) and their `cargo:` and
   `cargo::` instructions are applied.
4. Keeps every unit under a name derived from everything that decides it (the
   package, its features, its side, the units under it, the profile, the
   target and the compiler version). A unit found under that name is not
   compiled again. A path package is reused only while every file its
   compilation read (as `rustc`'s dep-info lists them), every file and
   variable its build script named with `rerun-if-changed` and
   `rerun-if-env-changed`, and every variable the compiler read are what they
   were; a change to a path package's files gives it, and everything above it,
   a new name.
5. Holds a lock on the output and cache directories for the whole build; a
   second build waits and says so.

## Refusals

Every refusal is printed as `kompilator: <cause>`; exit `2` for a usage
error, `1` for a build that could not be made.

| Cause | What to do |
|---|---|
| `kompilator build needs --manifest-path` / `kompilator where needs --manifest-path` | name the package's `Cargo.toml` |
| `kompilator takes build or where; got <word>` | the first word is the verb |
| `kompilator where does not take <flag>: it reads only --manifest-path, --release, --bin and --out, which decide where the binaries are` | drop the flag; it changes what is compiled, not where |
| `--bin <name>: <package> <version> declares no such binary; it declares <names>` | name one of the binaries the package declares |
| `<package> <version> declares no binary to build` | the package has no `[[bin]]` and no `src/main.rs`; it is a library |
| ``<name> has not been built: <path> does not exist; `<build command>` builds it there`` | run the command it names, then ask again |
| `the manifest <path> cannot be found: <error>` | name a manifest that exists |
| `<dir>/Cargo.lock cannot be read: <error>` | create the lock once with the registry's client |
| `<package> <version> comes from the registry <source>, and only crates.io is downloaded by this build; …` | unpack it once with that registry's own client |
| `the lock pins no checksum for <package> <version>, …` | the lock was written without checksums; lock again |
| `<url> could not be downloaded for <package> <version> (<status>): <curl's error>` | the network or crates.io is unreachable; curl's error says which |
| `<url> has SHA-256 <digest>, and the lock pins <checksum> for <package> <version>; …` | the download is not the locked package: do not build it |
| `<package> <version> did not unpack to <dir> (<status>): <tar's error>` | the archive is damaged or the disk is full |
| `<url> could not be fetched at <commit> for <package> <version>: …` | the URL is unreachable, the machine's Git credentials may not read it, or the commit was never pushed; Git's own words follow |
| `git could not run …; install Git` / `curl could not run …; install curl` | the tool is missing |
| `<package> depends on <dependency>, and its lock entry names no such package` | the lock does not match the manifest; lock again |
| `<package> <version> has no feature <feature>` | a manifest enables a feature its dependency does not declare |
| `--features <name>: <package> <version> has no feature <name>` | the package declares no such feature; its `[features]` table lists those it has |
| `--revision needs the manifest inside a Git repository: …` | the manifest is not in a Git checkout; build its working tree without `--revision` |
| `<rev> names no commit of <repository>; fetch it, or name a commit, branch or tag the repository holds` | the revision is not in this clone; fetch it first |
| `git archive <commit> in <repository> exited <status>: …` / `<commit> did not unpack into <dir> …` | the repository or the disk refused; their own words follow |
| `the profile setting <name> is not one this build reads` | a `[profile]` key this build does not apply |
| `the build script of <package> <version> exited <status>: <output>` | the script's own output says why |
| `rustc failed for <unit> (<status>): <diagnostics>` | the compiler's diagnostics say why |
| `rustc could not run in <dir>: <error>; install Rust or name the compiler with RUSTC` | no compiler is installed, or none is on `PATH` |
| `rustc --print sysroot in <dir> exited <status>: <output>` | the toolchain the package names is not installed; install it |
| `the toolchain at <sysroot> holds no bin/rustc` | the toolchain is broken; reinstall it |
| `compiling <package> again: <file> changed` | not a refusal: why a package of the workspace is compiled again |
| `another build holds <dir>/.lock; waiting for it` | not a refusal: the build waits for the other one |
