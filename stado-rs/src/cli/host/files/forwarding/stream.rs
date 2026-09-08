use sha2::{Digest, Sha256};

use crate::cli::CmdError;

use crate::cli::host::machine::releases::release_component;

/// The far side of a streamed delivery: verify, then let the file take its
/// name.
///
/// `@SUBDIR@` and `@MODE@` are the only things that vary between a credential
/// and any other delivered file, and the mode is written symbolically so the
/// contract reads as what it grants rather than as a number to decode.
const STREAM_FILE_BODY: &str = r#"dir="$HOME/@SUBDIR@"
staged="$dir/.$name.stado-stream"
trap 'rm -f "$staged"' EXIT
[ -s "$staged" ] || { printf '%s\n' 'delivered file is missing or empty' >&2; exit 1; }
/bin/chmod @MODE@ "$staged"
line=$(/usr/bin/openssl dgst -sha256 -r "$staged")
actual="${line%% *}"
if [ "$actual" != "$expected" ]; then
  printf '%s\n' 'transfer checksum mismatch' > /dev/stderr
  exit 1
fi
/bin/mv "$staged" "$dir/$name"
trap - EXIT
printf '%s\n' "$dir/$name"
"#;

/// Deliver one file too large to embed in a script, and verify it landed.
///
/// Same contract as the inline path -- owner-only, checksummed on the far side
/// before it takes the name -- with the bytes carried by the transport instead
/// of the command line. `subdir` and `mode` are what separate a credential
/// from any other delivered file; everything else about the delivery is
/// identical, which is why there is one of these rather than two.
pub(super) async fn stream_file(
    target: &str,
    source: &str,
    name: &str,
    subdir: &str,
    mode: &str,
) -> Result<(String, usize), CmdError> {
    release_component("delivered file name", name)?;
    let bytes = std::fs::metadata(source)?.len();
    let mut digest = Sha256::new();
    digest.update(std::fs::read(source)?);
    let expected_sha256 = hex::encode(digest.finalize());

    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let staged = format!("{subdir}/.{name}.stado-stream");

    let quoted_subdir = crate::deploy::shlex_quote(subdir);
    let prepare = crate::deploy::host_channel::run_script(
        &resolved,
        &format!(
            "set -euo pipefail\n/bin/mkdir -p \"$HOME\"/{quoted_subdir}\n\
             /bin/chmod u=rwx,go= \"$HOME\"/{quoted_subdir}\n"
        ),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !prepare.ok() {
        return Err(CmdError::click(format!(
            "{target}: cannot prepare the delivery directory: {}",
            crate::deploy::host_channel::last_error_line(&prepare, "remote mkdir failed")
        )));
    }

    if crate::deploy::host_channel::target_is_this_host(&resolved) {
        let home = std::env::var("HOME")
            .map_err(|_| CmdError::click("HOME is not set, so the secret path is unknown"))?;
        std::fs::copy(source, std::path::Path::new(&home).join(&staged))?;
    } else {
        let connection = crate::deploy::host_channel::select_ssh_connection(&resolved, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let ssh_target = connection.destination;
        let mut options = crate::deploy::host_channel::ssh_options(ssh_target);
        options.pop();
        let mut argv = vec!["scp".to_string(), "-q".to_string()];
        argv.extend(options.into_iter().skip(usize::from(true)));
        argv.push(source.to_string());
        argv.push(format!("{ssh_target}:{staged}"));
        let key = crate::deploy::ssh_key::materialize(resolved.channel_key())
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let argv = crate::deploy::ssh_key::add_identity(argv, &key)
            .map_err(|error| CmdError::click(error.to_string()))?;
        let copy = runner(crate::deploy::CommandSpec::new(argv))
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !copy.ok() {
            return Err(CmdError::click(format!(
                "{target}: cannot deliver the file: {}",
                copy.detail()
            )));
        }
    }

    let quoted_name = crate::deploy::shlex_quote(name);
    let quoted_sha = crate::deploy::shlex_quote(&expected_sha256);
    let script = format!(
        "set -euo pipefail\nname={quoted_name}\nexpected={quoted_sha}\n{}",
        STREAM_FILE_BODY
            .replace("@SUBDIR@", subdir)
            .replace("@MODE@", mode)
    );
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: delivery failed: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote secret write failed")
        )));
    }
    Ok((
        format!("$HOME/{subdir}/{name}"),
        usize::try_from(bytes).unwrap_or(usize::MAX),
    ))
}
