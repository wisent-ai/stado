//! Git as the submit queue uses it, in the repository's one checkout and
//! never in its working tree or index: fetch the remote `main`, write the
//! candidate's own commits again on its tip (`git merge-tree --write-tree`
//! and `git commit-tree`, author, date and message kept), and push the
//! result to `main` as a fast-forward. Every call runs with
//! `GIT_TERMINAL_PROMPT=0`, so a remote that wants a password refuses
//! instead of asking anyone.

use std::path::Path;

use crate::cli::CmdError;

/// The branch the queue lands on, a ref name of the forge protocol.
pub const MAIN: &str = "main";
/// The remote whose `main` the queue lands on.
pub const REMOTE: &str = "origin";

/// The remote a push goes to, as Git hands its pre-push hook two arguments:
/// the remote's name, or the URL itself when the push names none, and the
/// URL it pushes to (https://git-scm.com/docs/githooks#_pre_push).
pub struct Remote<'a> {
    pub name: &'a str,
    pub url: &'a str,
}

/// The repository `url` names, as `host/path` in lower case, so every
/// spelling Git accepts for one repository names it once: the scheme, a
/// user, a port, one `.git` and a trailing `/` are set aside, and the
/// scp-like `[user@]host:path` names the same as `ssh://[user@]host/path`
/// (https://git-scm.com/docs/git-push#_git_urls). A local path or a
/// `file://` URL names its path with no host. None for an empty URL. Case
/// is folded because forges name repositories without it; a case that does
/// tell two repositories apart only makes them one, which keeps the check.
pub fn named_repository(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url
        .strip_suffix(".git")
        .unwrap_or(url)
        .trim_end_matches('/');
    if url.is_empty() {
        return None;
    }
    let (authority, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').unwrap_or((rest, "")),
        None => match url.split_once(':') {
            Some((host, path)) if !host.contains('/') => (host, path),
            _ => ("", url),
        },
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split_once(':').map_or(host, |(host, _)| host);
    Some(format!("{host}/{}", path.trim_start_matches('/')).to_lowercase())
}

/// What one git call printed and how it ended.
pub struct Ran {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// `git args` in `checkout`, with `input` on its standard input.
pub async fn ran(checkout: &Path, args: &[&str], input: Option<&str>) -> Result<Ran, CmdError> {
    ran_with(checkout, args, input, &[]).await
}

/// [`ran`] with `env` set for git beside `GIT_TERMINAL_PROMPT`.
pub async fn ran_with(
    checkout: &Path,
    args: &[&str],
    input: Option<&str>,
    env: &[(&str, String)],
) -> Result<Ran, CmdError> {
    use tokio::io::AsyncWriteExt;
    let shown = format!("git {}", args.join(" "));
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(checkout)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(env.iter().map(|(name, value)| (*name, value.as_str())))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| CmdError::click(format!("{shown} did not start: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(input) = input {
            stdin.write_all(input.as_bytes()).await.map_err(|error| {
                CmdError::click(format!(
                    "{shown}: its standard input was not written: {error}"
                ))
            })?;
        }
    }
    let output = crate::wait::until(
        crate::wait::Kind::Process,
        &shown,
        checkout.display(),
        child.wait_with_output(),
    )
    .await
    .map_err(|error| CmdError::click(format!("{shown} did not run: {error}")))?;
    Ok(Ran {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// `git args` in `checkout`: its trimmed standard output, or a refusal
/// naming the command, the checkout and git's own words.
pub async fn git(checkout: &Path, args: &[&str]) -> Result<String, CmdError> {
    let ran = ran(checkout, args, None).await?;
    if !ran.success {
        return Err(CmdError::click(format!(
            "git {} in {} failed: {}",
            args.join(" "),
            checkout.display(),
            ran.stderr.trim()
        )));
    }
    Ok(ran.stdout.trim().to_string())
}

/// `owner/name` of the checkout's `origin`: the last two components of the
/// address it declares (`remote.origin.url`), one `.git` removed, whatever
/// the transport. `git remote get-url` is not read: it applies the host's
/// `url.<base>.insteadOf` rewrites, and an SSH alias it rewrites to names
/// no repository. The two components name the queue's directory of the
/// repository, so `.` and `..` are refused.
pub async fn repository(checkout: &Path) -> Result<String, CmdError> {
    let key = format!("remote.{REMOTE}.url");
    let url = git(checkout, &["config", "--get", &key])
        .await
        .map_err(|error| {
            error.within(format!(
                "{} has no {REMOTE} remote, and the submit queue lands on {REMOTE}'s {MAIN}",
                checkout.display()
            ))
        })?;
    let trimmed = url.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let mut parts = trimmed.rsplit(['/', ':']);
    let named = |part: &&str| !matches!(*part, "" | "." | "..");
    match (parts.next().filter(named), parts.next().filter(named)) {
        (Some(name), Some(owner)) => Ok(format!("{owner}/{name}")),
        _ => Err(CmdError::declaration(format!(
            "{}'s {REMOTE} is {url}, which names no owner/name repository",
            checkout.display()
        ))),
    }
}

/// The full commit `revision` names in `checkout`, fetched from `origin` by
/// its id when the checkout does not hold it yet. A revision beginning with
/// `-` is refused: git would read it as an option of `git fetch`.
pub async fn commit(checkout: &Path, revision: &str) -> Result<String, CmdError> {
    if revision.is_empty() || revision.starts_with('-') {
        return Err(CmdError::usage(format!(
            "--commit {revision:?} names no commit; give its id or a ref the checkout holds"
        )));
    }
    let spec = format!("{revision}^{{commit}}");
    if let Ok(full) = git(checkout, &["rev-parse", "--verify", "--quiet", &spec]).await {
        return Ok(full);
    }
    let fetched = ran(checkout, &["fetch", "--quiet", REMOTE, revision], None).await?;
    match git(checkout, &["rev-parse", "--verify", "--quiet", &spec]).await {
        Ok(full) => Ok(full),
        Err(_) => Err(CmdError::missing(format!(
            "{} holds no commit {revision}, and {REMOTE} did not give it ({}); push it to a \
             branch of {REMOTE} or commit it in this checkout, then submit it again",
            checkout.display(),
            fetched.stderr.trim()
        ))),
    }
}

/// Fetch `origin`'s `main` and answer the commit it names.
pub async fn fetch_main(checkout: &Path) -> Result<String, CmdError> {
    let refspec = format!("+refs/heads/{MAIN}:refs/remotes/{REMOTE}/{MAIN}");
    git(checkout, &["fetch", "--quiet", REMOTE, &refspec])
        .await
        .map_err(|error| error.stating(crate::primitives::failure::FailureCode::InfraDown))?;
    git(
        checkout,
        &["rev-parse", &format!("refs/remotes/{REMOTE}/{MAIN}")],
    )
    .await
}

/// Whether `ancestor` is in `descendant`'s history.
pub async fn contains(checkout: &Path, ancestor: &str, descendant: &str) -> Result<bool, CmdError> {
    let ran = ran(
        checkout,
        &["merge-base", "--is-ancestor", ancestor, descendant],
        None,
    )
    .await?;
    Ok(ran.success)
}

/// The paths `commit` changes against `base`, as the repository spells them
/// (`-z`, so a path Git would quote is read as it is), and the lines it adds
/// and removes; a binary file counts no line.
pub async fn changed(
    checkout: &Path,
    base: &str,
    commit: &str,
) -> Result<(Vec<String>, usize), CmdError> {
    let listed = git(
        checkout,
        &["diff", "--numstat", "-z", "--no-renames", base, commit],
    )
    .await?;
    let mut paths = Vec::new();
    let mut lines = 0;
    for row in listed.split('\0') {
        let mut fields = row.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        lines += counted(added, path)? + counted(removed, path)?;
        paths.push(path.to_string());
    }
    Ok((paths, lines))
}

/// One `--numstat` count of `path`: `-` is a binary file's, no line.
fn counted(field: &str, path: &str) -> Result<usize, CmdError> {
    if field == "-" {
        return Ok(0);
    }
    field.parse().map_err(|error| {
        CmdError::click(format!(
            "git diff --numstat counted {field:?} lines of {path}, which is no count: {error}"
        ))
    })
}

/// How a push to `main` ended.
pub enum Pushed {
    Landed,
    /// `main` moved since it was fetched; the candidate is written again.
    Moved,
    /// The remote or a push hook refused, in its own words.
    Refused(String),
}

/// Push `commit` to `origin`'s `main` as a fast-forward; never forced.
pub async fn push(checkout: &Path, commit: &str) -> Result<Pushed, CmdError> {
    let refspec = format!("{commit}:refs/heads/{MAIN}");
    let ran = ran(checkout, &["push", "--porcelain", REMOTE, &refspec], None).await?;
    if ran.success {
        return Ok(Pushed::Landed);
    }
    let rejected = ran.stdout.lines().any(|line| {
        line.starts_with('!') && (line.contains("fetch first") || line.contains("non-fast-forward"))
    });
    if rejected {
        return Ok(Pushed::Moved);
    }
    Ok(Pushed::Refused(format!(
        "{}\n{}",
        ran.stdout.trim(),
        ran.stderr.trim()
    )))
}
