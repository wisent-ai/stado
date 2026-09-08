//! What a recipe is read as: the POSIX-quoted shell program one build job
//! runs, and the name of the file that program writes the built commit's
//! exact tag into.
//!
//! Shared verbatim with `stado builds run`, so the operator's manual enqueue
//! and the poller submit byte-identical commands.

use crate::targets::BuildRecipe;

/// POSIX single-quote `value` so operator-entered recipe fields (repo, ref,
/// command, artifact paths) can never terminate or extend the generated
/// shell chain.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Name of the file every build writes at the root of its uploaded output:
/// the exact tag the built commit carries, or nothing at all.
///
/// The version of a build is a property of the commit, and only the machine
/// holding the checkout can read it. Resolving it later from the recipe's
/// branch answers for whatever the branch points at by then, which is how a
/// build of one commit gets declared as the version of another.
pub const BUILD_VERSION_FILE: &str = "stado-build-version.txt";

/// The full command a build job runs, from the agent's job workdir: shallow
/// single-branch clone into a self-cleaning temp dir, the built commit's exact
/// tag recorded in [`BUILD_VERSION_FILE`], the recipe's build command inside
/// the checkout, then each declared artifact copied (tree structure
/// preserved) into the workdir's `output/` — the directory the agent already
/// uploads under the job's canonical results prefix
/// (`status/<job_id>/output/`, plus any `output_uri` mirror).
///
/// The tag is read before the build runs: a build command is free to check
/// out, fetch or tag inside the checkout, and the version being recorded is
/// the one that was cloned. `git describe --exact-match` prints nothing and
/// fails for an untagged commit, which is most commits — the redirect still
/// creates the file, and an empty file is the answer "this commit carries no
/// version" rather than a missing upload nobody can distinguish from a build
/// that never got that far.
///
/// Shared with `stado builds run`, so the operator's manual enqueue and the
/// poller submit byte-identical commands.
pub fn build_job_command(recipe: &BuildRecipe) -> String {
    let mut command = format!(
        "set -eu; root=\"$PWD\"; src=\"$(mktemp -d)\"; trap 'rm -rf \"$src\"' EXIT; \
         git clone --depth 1 --branch {branch} -- {repo} \"$src/checkout\"; \
         cd \"$src/checkout\"; mkdir -p \"$root/output\"; \
         {{ git describe --exact-match --tags HEAD 2>/dev/null || :; }} \
         > \"$root/output/{version}\"; \
         sh -c {build}",
        branch = sh_quote(&recipe.branch),
        repo = sh_quote(&recipe.repo),
        version = BUILD_VERSION_FILE,
        build = sh_quote(&recipe.command),
    );
    for artifact in &recipe.artifacts {
        let quoted = sh_quote(artifact);
        command.push_str(&format!(
            "; mkdir -p \"$(dirname \"$root/output/\"{quoted})\"; \
             cp -R {quoted} \"$root/output/\"{quoted}"
        ));
    }
    command
}
