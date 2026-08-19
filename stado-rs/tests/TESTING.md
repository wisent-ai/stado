# Testing guidelines

How tests are written in this repository. Distilled from the operator's
instructions on 2026-08-19; when a habit here conflicts with an instruction
in the conversation, the conversation wins.

## Where tests live

1. Tests live in the product repository, not in Probierz and not in
   external tooling. Probierz stays for evidence runs; it is not the home
   of tests.
2. Layout is `tests/<area>/` — one folder per domain (`tests/fleet/`),
   extended with new areas over time, never one flat bag.
3. No inline tests in `src` — production code carries no tests inside it.

## What a test covers

4. A test drives the real product binary end to end
   (`CARGO_BIN_EXE_stado` via `Command`), not mocks and not internals.
   The contract under test is the command's behavior from the outside.
5. Assertions read state, not output: the `registry.json` document on
   disk, the exit code, the exact refusal sentences. Stdout is supporting
   evidence, never the only one.
6. Every command is a set of paths: success + refusals (duplicate,
   malformed name, unknown object, occupied state) — each refusal with
   its exact sentence, because that sentence is part of the contract.
7. One test = one command and its story (create with its refusals,
   assign with its refusals, delete with its refusals). The test name
   says what it defends.

## How a test is built

8. Isolation through the environment: a tempdir plus
   `WC_STORAGE_BACKEND=local`, `WC_LOCAL_STORAGE_PATH`, and a
   `STADO_CONFIG` pointing at a nonexistent file. A test never touches
   the operator's real registry, vault, or configuration. Commands that
   write to the real vault (e.g. invite minting) are out of test scope
   unless a dedicated fixture exists.
9. Before a test is written, the behavior is probed by hand against a
   seeded state — exit codes and error sentences are copied from live
   answers, never guessed.
10. If a command does not exist, the command is implemented first and
    the test second — a test never describes fiction.
11. Tests are not run without an explicit instruction. Verification is
    compilation (`cargo test --no-run`) and manual probes of the product
    commands.

## What not to do

12. Never create files beyond what was asked — no helper scripts,
    generators, or one-off tooling.
13. Never delete another agent's work, and never run ahead of scope:
    folders means folders, a file means a file.
