use super::{checked_version, observed, program, Prepared, VERSION};
use crate::deploy::{host_channel, host_delivery, shlex_quote, Runner};
use crate::targets::ComputeTarget;
use anyhow::{bail, ensure, Result};

async fn verify(
    target: &ComputeTarget,
    path: &str,
    prepared: &Prepared,
    runner: &Runner,
) -> Result<()> {
    let quoted = shlex_quote(path);
    ensure!(
        host_channel::remote_test(
            target,
            &format!("-f {quoted} -a ! -L {quoted} -a -O {quoted}"),
            runner
        )
        .await?,
        "native SDK is absent, symlinked or not owned by the approved account: {path}"
    );
    let digest =
        host_channel::run_program(target, &["/usr/bin/shasum", "-a", "256", path], runner).await?;
    ensure!(
        digest.ok(),
        "native SDK checksum failed for {path}, exit {}: {}",
        digest.code,
        digest.detail()
    );
    ensure!(
        digest.stdout.split_whitespace().next() == Some(prepared.receipt.executable_sha256.as_str()),
        "native SDK bytes differ on {}: {path}; expected sha256 {}, observed {:?}; no unverified executable was run",
        target.name, prepared.receipt.executable_sha256, digest.stdout
    );
    let output = host_channel::run_program(target, &[path, "--version"], runner).await?;
    checked_version(&output, &prepared.receipt)
}

fn preparation(home: &str, program: &str, stage: &str) -> Result<String> {
    ensure!(
        home.starts_with('/')
            && home
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'/' | b'.' | b'_' | b'-')),
        "the approved account home cannot be represented safely by the Stado rsync transport"
    );
    let parents = [
        home.to_owned(),
        format!("{home}/.stado"),
        format!("{home}/.stado/cache"),
        format!("{home}/.stado/cache/product-sdk"),
        format!("{home}/.stado/cache/product-sdk/{VERSION}"),
        format!("{home}/.stado/cache/product-sdk/{VERSION}/darwin-arm64"),
    ]
    .iter()
    .map(|path| shlex_quote(path))
    .collect::<Vec<_>>()
    .join(" ");
    Ok(format!(
        "set -eu\numask 077\n\
         [ -d {home} ] && [ ! -L {home} ] || {{ printf '%s\\n' 'approved HOME is absent or symlinked' >&2; exit 1; }}\n\
         for directory in {parents}; do\n\
           [ ! -L \"$directory\" ] || {{ printf 'SDK cache traverses a symlink: %s\\n' \"$directory\" >&2; exit 1; }}\n\
           /bin/mkdir -p \"$directory\"\n\
           [ -d \"$directory\" ] && [ ! -L \"$directory\" ] && [ -O \"$directory\" ] || {{ printf 'SDK cache parent is not an owned directory: %s\\n' \"$directory\" >&2; exit 1; }}\n\
           mode=$(/usr/bin/stat -f '%Lp' \"$directory\")\n\
           [ $((8#$mode & 0022)) -eq 0 ] || {{ printf 'SDK cache parent is writable by another account: %s\\n' \"$directory\" >&2; exit 1; }}\n\
         done\n\
         if [ -e {program} ] || [ -L {program} ]; then printf '%s\\n' present; exit 0; fi\n\
         set -C\n: > {stage}\nprintf '%s\\n' absent\n",
        home = shlex_quote(home), program = shlex_quote(program), stage = shlex_quote(stage)
    ))
}

pub(super) async fn install(
    target: &ComputeTarget,
    home: &str,
    prepared: &Prepared,
    runner: &Runner,
) -> Result<String> {
    let program = program(home, "darwin-arm64");
    let stage = format!("{program}.incoming-{}", uuid::Uuid::new_v4());
    let output =
        host_channel::run_script(target, &preparation(home, &program, &stage)?, runner).await?;
    ensure!(
        output.ok(),
        "native SDK host preflight failed, exit {}: {}",
        output.code,
        output.detail()
    );
    match output.stdout.trim() {
        "present" => {
            verify(target, &program, prepared, runner).await?;
            observed(prepared, &program);
            return Ok(program);
        }
        "absent" => {}
        other => bail!("native SDK host preflight returned an unknown state: {other:?}"),
    }
    let result: Result<()> = async {
        host_delivery::transfer_native_sdk(target, &prepared.program, &stage, runner).await?;
        verify(target, &stage, prepared, runner).await?;
        let commit = host_channel::run_program(target, &["/bin/ln", &stage, &program], runner).await?;
        if let Err(error) = verify(target, &program, prepared, runner).await {
            bail!(
                "native SDK commit did not produce the qualified executable: ln exit {}, stdout {:?}, stderr {:?}; observed {error:#}",
                commit.code, commit.stdout, commit.stderr
            );
        }
        Ok(())
    }.await;
    let cleanup: Result<()> = async {
        let removed = host_channel::run_program(target, &["/bin/rm", "-f", &stage], runner).await?;
        ensure!(
            removed.ok(),
            "cannot remove native SDK staging {stage}, exit {}: {}",
            removed.code,
            removed.detail()
        );
        Ok(())
    }
    .await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => {
            observed(prepared, &program);
            Ok(program)
        }
        (Err(error), Err(cleanup)) => Err(error.context(format!(
            "native SDK staging cleanup also failed: {cleanup:#}"
        ))),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
    }
}
