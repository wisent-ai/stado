//! The privacy requests macOS recorded against one program on a host, read
//! from that host's unified log.
//!
//! macOS answers every access to a protected location (the Files and Folders
//! folders, Photos, the media library, Contacts, other applications' data)
//! in `tccd`, which logs each request in four lines sharing one message id:
//! the service and whether it was only a check (`AUTHREQ_CTX`), the program
//! that accessed and the one responsible for it (`AUTHREQ_ATTRIBUTION`), the
//! answer (`AUTHREQ_RESULT`) and, when nobody had decided yet, the dialog
//! shown to the person at the keyboard (`AUTHREQ_PROMPTING`). The host joins
//! the four by message id and sends back only the requests whose accessing
//! or responsible program is the one asked about, so a day of every
//! program's requests never crosses the connection. Local Network, which
//! macOS decides outside TCC, is read by [`local_network`].

pub mod local_network;

use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The report line one request is sent back as.
const REQUEST_MARK: &str = "STADO_TCC_REQUEST";
/// The line saying the log was read to its end.
const END_MARK: &str = "STADO_TCC_END";
/// The line saying `log show` itself failed, with its exit status.
const FAILED_MARK: &str = "STADO_TCC_FAILED";
/// The line saying the host has no unified log (it is not a Mac).
const UNSUPPORTED_MARK: &str = "STADO_TCC_UNSUPPORTED";

/// The `sandboxd` identifier: a request it makes is the program opening a
/// protected location, as opposed to `syspolicyd`'s check of a program it
/// was about to start.
pub const SANDBOX_REQUESTER: &str = "com.apple.sandboxd";
/// The Full Disk Access check `sandboxd` makes before every narrower one; on
/// its own it never shows a dialog.
pub const ALL_FILES_SERVICE: &str = "kTCCServiceSystemPolicyAllFiles";

/// The `targets[].privacy_grants` folder key a Files and Folders service
/// stands for, when it stands for one.
pub fn folder_of_service(service: &str) -> Option<&'static str> {
    match service {
        "kTCCServiceSystemPolicyDocumentsFolder" => Some("documents"),
        "kTCCServiceSystemPolicyDesktopFolder" => Some("desktop"),
        "kTCCServiceSystemPolicyDownloadsFolder" => Some("downloads"),
        _ => None,
    }
}

/// One request `tccd` recorded against the program.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrivacyRequest {
    /// The host's local time of the request, as the log states it.
    pub at: String,
    /// The `kTCCService…` name of what was asked for.
    pub service: String,
    /// Whether the request was only a check (`preflight=yes`), which never
    /// shows a dialog, rather than an access that may.
    pub check_only: bool,
    /// `tccd`'s `authValue`: 0 denied, 2 allowed, 3 limited; none when the
    /// log holds no answer.
    pub answer: Option<String>,
    /// Whether macOS showed the person at the keyboard a dialog for it.
    pub prompted: bool,
    /// The executable that made the access.
    pub accessing: String,
    /// The executable macOS held responsible, when it is another one than
    /// the accessing program (a child Stado started).
    pub responsible: Option<String>,
    /// Who asked `tccd`: `com.apple.sandboxd` for an open of a protected
    /// location, `com.apple.syspolicyd` for its developer-tool check of a
    /// program being started.
    pub requested_by: String,
}

impl PrivacyRequest {
    /// Whether the request reached for protected data: an open `sandboxd`
    /// reported, or any request that showed a dialog.
    pub fn reads_protected_data(&self) -> bool {
        self.prompted || self.requested_by == SANDBOX_REQUESTER
    }

    /// Whether the request is one no declaration in `declared` (folder keys
    /// the host declares for the program) accounts for: it reached for
    /// protected data, and it was neither a declared folder nor the Full Disk
    /// Access check that precedes every protected open.
    pub fn undeclared(&self, declared: &std::collections::BTreeSet<String>) -> bool {
        if !self.reads_protected_data() || self.service == ALL_FILES_SERVICE {
            return false;
        }
        !folder_of_service(&self.service).is_some_and(|folder| declared.contains(folder))
    }
}

/// One request as a line: when, what, whether it was only a check and
/// whether it showed a dialog, what macOS answered, who accessed (for whom),
/// and who asked `tccd`.
impl std::fmt::Display for PrivacyRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = if self.check_only { "check" } else { "access" };
        let prompted = if self.prompted { " prompted" } else { "" };
        write!(formatter, "{} {} {kind}{prompted} ", self.at, self.service)?;
        match &self.answer {
            Some(answer) => write!(formatter, "answer {answer}")?,
            None => write!(formatter, "no answer logged")?,
        }
        write!(formatter, " by {}", self.accessing)?;
        if let Some(responsible) = &self.responsible {
            write!(formatter, " for {responsible}")?;
        }
        write!(formatter, " via {}", self.requested_by)
    }
}

/// Whether `since` is a window `log show --last` accepts: a positive whole
/// number followed by `m`, `h` or `d`.
pub fn valid_window(since: &str) -> bool {
    match since.char_indices().last() {
        Some((unit_at, 'm' | 'h' | 'd')) => {
            let count = &since[..unit_at];
            !count.is_empty()
                && count.chars().all(|digit| digit.is_ascii_digit())
                && count.chars().any(|digit| digit != '0')
        }
        _ => false,
    }
}

/// The program the host runs: `tccd`'s requests of the last `since`, joined
/// by message id, those attributed to `program` sent back one per line.
fn script(program: &str, since: &str) -> String {
    format!(
        r#"if [ ! -x /usr/bin/log ]; then
  printf '{UNSUPPORTED_MARK}\n'
  exit 0
fi
{{ /usr/bin/log show --last {since} --style compact --predicate 'subsystem == "com.apple.TCC" AND category == "access" AND eventMessage BEGINSWITH "AUTHREQ_"'; printf 'STADO_LOG_STATUS %s\n' "$?"; }} |
  STADO_PROGRAM={program} /usr/bin/awk '
    function value(text, name,   start, rest) {{
      start = index(text, name "=")
      if (start == 0) return ""
      rest = substr(text, start + length(name) + 1)
      if (match(rest, /[,}}]/)) rest = substr(rest, 1, RSTART - 1)
      return rest
    }}
    function party(text, role,   start, rest) {{
      start = index(text, role "={{TCCDProcess:")
      if (start == 0) return ""
      rest = substr(text, start)
      if (match(rest, /}}/)) rest = substr(rest, 1, RSTART)
      return rest
    }}
    /^STADO_LOG_STATUS / {{ status = $2; next }}
    {{
      if (!match($0, /tccd\[[0-9]+/)) next
      daemon = substr($0, RSTART + 5, RLENGTH - 5)
      id = value($0, "msgID")
      if (id == "") next
      key = daemon ":" id
      if (index($0, "AUTHREQ_CTX:")) {{
        at[key] = $1 " " $2
        service[key] = value($0, "service")
        preflight[key] = value($0, "preflight")
      }} else if (index($0, "AUTHREQ_ATTRIBUTION:")) {{
        responsible = value(party($0, "responsible"), "binary_path")
        accessing = value(party($0, "accessing"), "binary_path")
        if (responsible == ENVIRON["STADO_PROGRAM"] || accessing == ENVIRON["STADO_PROGRAM"]) {{
          mine[key] = 1
          who[key] = accessing
          held[key] = responsible
          by[key] = value(party($0, "requesting"), "identifier")
        }}
      }} else if (index($0, "AUTHREQ_RESULT:")) {{
        answer[key] = value($0, "authValue")
      }} else if (index($0, "AUTHREQ_PROMPTING:")) {{
        prompted[key] = "yes"
      }}
    }}
    END {{
      if (status != "0") {{
        printf "{FAILED_MARK}\t%s\n", status
        exit 0
      }}
      for (key in mine)
        printf "{REQUEST_MARK}\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", at[key], service[key], preflight[key], answer[key], prompted[key], who[key], held[key], by[key]
      print "{END_MARK}"
    }}'
"#,
        program = shlex_quote(program),
    )
}

/// The requests the script's report lines carry, oldest first; a report
/// that does not say the log was read to its end, or says `log show`
/// failed, is the error naming what the host wrote.
fn parse(stdout: &str, stderr: &str) -> Result<Vec<PrivacyRequest>, String> {
    let mut requests = Vec::new();
    let mut ended = false;
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            [UNSUPPORTED_MARK] => {
                return Err(
                    "the host has no /usr/bin/log: it is not a Mac, and only macOS \
                            records privacy requests"
                        .to_string(),
                )
            }
            [FAILED_MARK, status] => {
                return Err(format!(
                    "`log show` exited {status} on the host: {}",
                    stderr.trim()
                ))
            }
            [END_MARK] => ended = true,
            [REQUEST_MARK, at, service, preflight, answer, prompted, accessing, responsible, by] => {
                requests.push(PrivacyRequest {
                    at: at.to_string(),
                    service: service.to_string(),
                    check_only: *preflight == "yes",
                    answer: (!answer.is_empty()).then(|| answer.to_string()),
                    prompted: *prompted == "yes",
                    accessing: accessing.to_string(),
                    responsible: (!responsible.is_empty() && responsible != accessing)
                        .then(|| responsible.to_string()),
                    requested_by: by.to_string(),
                })
            }
            _ => {}
        }
    }
    if !ended {
        return Err(format!(
            "the host's privacy request read ended without its end line: {}",
            stderr.trim()
        ));
    }
    requests.sort_by(|left, right| left.at.cmp(&right.at));
    Ok(requests)
}

/// The privacy requests `target`'s unified log recorded against `program`
/// in the last `since` (a `log show --last` window), read on the host
/// through its declared connection.
pub async fn requests_on_host(
    target: &ComputeTarget,
    program: &str,
    since: &str,
    runner: &Runner,
) -> Result<Vec<PrivacyRequest>, DeployError> {
    if !valid_window(since) {
        return Err(DeployError(format!(
            "--since {since:?} is not a window `log show --last` reads: a positive whole number \
             followed by m, h or d (for example 1d)"
        )));
    }
    let output = host_channel::run_script(target, &script(program, since), runner).await?;
    if !output.succeeded() {
        return Err(DeployError(format!(
            "the privacy request read on {} exited {}: {}",
            target.name,
            output.code,
            output.detail().trim()
        )));
    }
    parse(&output.stdout, &output.stderr)
        .map_err(|cause| DeployError(format!("{}: {cause}", target.name)))
}
