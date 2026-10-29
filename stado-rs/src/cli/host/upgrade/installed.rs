//! The Stado a host runs, read on the host through its declared connection:
//! the program's digest, the version it reports, and the code signature macOS
//! keeps its privacy decisions under, for the installed program and for the
//! rollback target an install keeps beside it.

use serde::Serialize;

use crate::cli::CmdError;
use crate::deploy::{host_channel, Runner};
use crate::targets::ComputeTarget;

const HOME: &str = "STADO_UPGRADE_HOME";
const SYSTEM: &str = "STADO_UPGRADE_SYSTEM";
const ABSENT: &str = "STADO_UPGRADE_ABSENT";
const DIGEST: &str = "STADO_UPGRADE_DIGEST";
const SIGNATURE: &str = "STADO_UPGRADE_SIGNATURE";
const VERIFIED: &str = "STADO_UPGRADE_VERIFIED";
const VERSION: &str = "STADO_UPGRADE_VERSION";
const VERSION_FAILED: &str = "STADO_UPGRADE_VERSION_FAILED";
const END: &str = "STADO_UPGRADE_END";

/// The installed name and the rollback target install keeps beside it.
pub(super) const INSTALLED: &str = "stado";
pub(super) const KEPT: &str = "stado.previous";

/// What `codesign` says about one program on the host.
#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct Signature {
    /// `stable` (an Apple Development or Developer ID identity), `adhoc`,
    /// `unsigned`, `invalid`, `untrusted`, or `not_applicable` off macOS.
    pub state: String,
    pub identifier: Option<String>,
    pub team: Option<String>,
    pub authority: Option<String>,
    pub requirement: Option<String>,
}

/// One program under `~/.stado/bin` on the host.
#[derive(Clone, Debug, Serialize)]
pub(super) struct Program {
    pub path: String,
    pub sha256: String,
    pub signature: Signature,
}

/// The Stado the host runs.
#[derive(Clone, Debug, Serialize)]
pub(super) struct Installed {
    pub home: String,
    pub darwin: bool,
    /// What `stado --version` printed there.
    pub version_line: String,
    pub version: String,
    /// The source revision the build reports, without `-dirty`.
    pub revision: Option<String>,
    /// The build was made from a tree with uncommitted edits.
    pub dirty: bool,
    pub program: Program,
    /// The rollback target, when the host keeps one.
    pub kept: Option<Program>,
}

fn script() -> String {
    format!(
        r#"set -u
bin="$HOME/.stado/bin"
printf '{HOME}\t%s\n' "$HOME"
printf '{SYSTEM}\t%s\n' "$(/usr/bin/uname -s)"
for name in {INSTALLED} {KEPT}; do
  path="$bin/$name"
  if [ -L "$path" ] || [ ! -f "$path" ]; then printf '{ABSENT}\t%s\n' "$name"; continue; fi
  digest=$(/usr/bin/openssl dgst -sha256 -r "$path") || continue
  printf '{DIGEST}\t%s\t%s\n' "$name" "${{digest%% *}}"
  if [ -x /usr/bin/codesign ]; then
    /usr/bin/codesign --display --verbose=4 -r- "$path" 2>&1 | while IFS= read -r line; do
      printf '{SIGNATURE}\t%s\t%s\n' "$name" "$line"
    done
    if /usr/bin/codesign --verify --strict "$path" 2>/dev/null; then verified=yes; else verified=no; fi
    printf '{VERIFIED}\t%s\t%s\n' "$name" "$verified"
  fi
done
if version=$("$bin/{INSTALLED}" --version 2>&1); then
  printf '{VERSION}\t%s\n' "$version"
else
  printf '{VERSION_FAILED}\t%s\n' "$version" | /usr/bin/head -n 1
fi
printf '{END}\n'
"#
    )
}

/// The signature state `inspect_signature` would give the same `codesign`
/// display, read from its lines.
fn signature(lines: &[&str], verified: bool, darwin: bool) -> Signature {
    if !darwin {
        return Signature {
            state: "not_applicable".to_string(),
            ..Signature::default()
        };
    }
    let field = |name: &str| {
        lines.iter().find_map(|line| {
            line.split_once('=')
                .filter(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        })
    };
    let requirement = lines.iter().find_map(|line| {
        line.split_once("designated => ")
            .map(|(_, value)| value.to_string())
    });
    let mut found = Signature {
        state: String::new(),
        identifier: field("Identifier"),
        team: field("TeamIdentifier").filter(|team| team != "not set"),
        authority: field("Authority"),
        requirement,
    };
    found.state = if found.identifier.is_none() {
        "unsigned"
    } else if field("Signature").as_deref() == Some("adhoc")
        || found
            .requirement
            .as_deref()
            .is_some_and(|requirement| requirement.contains("cdhash "))
    {
        "adhoc"
    } else if !verified {
        "invalid"
    } else if found.team.is_some()
        && found.authority.as_deref().is_some_and(|authority| {
            authority.starts_with("Developer ID Application:")
                || authority.starts_with("Apple Development:")
        })
    {
        "stable"
    } else {
        "untrusted"
    }
    .to_string();
    found
}

/// Read the Stado `target` runs.
pub(super) async fn read(target: &ComputeTarget, runner: &Runner) -> Result<Installed, CmdError> {
    let output = host_channel::run_script(target, &script(), runner)
        .await
        .map_err(CmdError::from)?;
    let name = &target.name;
    if !output.stdout.lines().any(|line| line == END) {
        return Err(CmdError::unreachable(format!(
            "{name}: the read of its installed Stado ended without its end line (exit {}): {}",
            output.code,
            output.stderr.trim()
        )));
    }
    let mut home = None;
    let mut darwin = false;
    let mut version_line = None;
    let mut version_failed = None;
    let mut digests: Vec<(String, String)> = Vec::new();
    let mut signatures: Vec<(String, String)> = Vec::new();
    let mut verified: Vec<(String, bool)> = Vec::new();
    for line in output.stdout.lines() {
        match line.splitn(3, '\t').collect::<Vec<_>>().as_slice() {
            [HOME, value] => home = Some(value.to_string()),
            [SYSTEM, value] => darwin = *value == "Darwin",
            [DIGEST, program, digest] => digests.push((program.to_string(), digest.to_string())),
            [SIGNATURE, program, text] => signatures.push((program.to_string(), text.to_string())),
            [VERIFIED, program, answer] => verified.push((program.to_string(), *answer == "yes")),
            [VERSION, value] => version_line = Some(value.trim().to_string()),
            [VERSION_FAILED, value] => version_failed = Some(value.trim().to_string()),
            _ => {}
        }
    }
    let home = home.ok_or_else(|| {
        CmdError::unreachable(format!("{name}: the remote shell did not state its home"))
    })?;
    let program = |wanted: &str| -> Option<Program> {
        let (_, sha256) = digests.iter().find(|(program, _)| program == wanted)?;
        let lines: Vec<&str> = signatures
            .iter()
            .filter(|(program, _)| program == wanted)
            .map(|(_, line)| line.as_str())
            .collect();
        let verified = verified
            .iter()
            .any(|(program, answer)| program == wanted && *answer);
        Some(Program {
            path: format!("{home}/.stado/bin/{wanted}"),
            sha256: sha256.clone(),
            signature: signature(&lines, verified, darwin),
        })
    };
    let installed = program(INSTALLED).ok_or_else(|| {
        CmdError::missing(format!(
            "{name} has no Stado at {home}/.stado/bin/{INSTALLED}; an upgrade replaces an \
             installed Stado, and a host without one is enrolled (`stado fleet enroll`)"
        ))
    })?;
    let version_line = version_line.ok_or_else(|| {
        CmdError::unreachable(format!(
            "{name}: its installed Stado did not report a version: {}",
            match version_failed {
                Some(said) => said,
                None => "it printed nothing".to_string(),
            }
        ))
    })?;
    let (version, revision, dirty) = parse_version(&version_line).ok_or_else(|| {
        CmdError::unreachable(format!(
            "{name}: its installed Stado reported {version_line:?}, not `stado VERSION (rev \
             COMMIT)`"
        ))
    })?;
    Ok(Installed {
        kept: program(KEPT),
        home,
        darwin,
        version_line,
        version,
        revision,
        dirty,
        program: installed,
    })
}

/// `stado 0.23.67 (rev <commit>[-dirty])` as its version, its revision
/// without `-dirty`, and whether it was dirty.
fn parse_version(line: &str) -> Option<(String, Option<String>, bool)> {
    let mut words = line.split_whitespace();
    words.next()?;
    let version = words.next()?.to_string();
    let revision = line
        .split_once("(rev ")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(revision, _)| revision.trim().to_string())
        .filter(|revision| !revision.is_empty());
    let dirty = revision
        .as_deref()
        .is_some_and(|revision| revision.ends_with("-dirty"));
    let revision = revision.map(|revision| revision.trim_end_matches("-dirty").to_string());
    Some((version, revision, dirty))
}
