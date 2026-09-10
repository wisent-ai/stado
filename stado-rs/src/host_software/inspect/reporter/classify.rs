//! What each population path is, decided before anything is read from it.

use crate::deploy::{host_channel, shlex_quote, DeployError};

use super::Reporter;

/// What one population path is, decided without executing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::host_software::inspect) enum Classification {
    /// Not a regular file, staging litter, or a non-script that is not
    /// executable: not part of the report.
    Ignored,
    /// A shebang file: counted, never rowed.
    Script,
    /// An executable that is not a script: rowed, with its digest, provenance
    /// and version.
    Program,
}

/// What a path's name alone decides, before the host is asked anything.
///
/// A `.previous` is the rollback copy of a program already reported under
/// its own name, and a dotfile is this directory's own staging litter.
fn classify_name(path: &str) -> Option<Classification> {
    if path.is_empty() {
        return Some(Classification::Ignored);
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    (base.starts_with('.') || base.ends_with(".previous")).then_some(Classification::Ignored)
}

/// The classification `find` printed for one entry: `<name>\t<x|->\t<hex>`.
fn classify_line(dir: &str, line: &str) -> Option<(String, Classification)> {
    let mut fields = line.splitn(3, '\t');
    let (name, mode, head) = (fields.next()?, fields.next()?, fields.next()?);
    let name = name.strip_prefix("./").unwrap_or(name);
    if name.is_empty() {
        return None;
    }
    let path = format!("{dir}/{name}");
    let decided = classify_name(&path).unwrap_or(if head == "2321" {
        Classification::Script
    } else if mode == "x" {
        Classification::Program
    } else {
        Classification::Ignored
    });
    Some((path, decided))
}

impl Reporter<'_> {
    /// The regular-file check, the two-byte read and the executable bit for
    /// one path, as one classification.
    ///
    /// A shebang is tested before the executable bit and not after: retired
    /// helpers on control-host are no longer executable, but their shebangs
    /// are still the population the report is contracted to count. What is
    /// left has to be executable to be a program — `$HOME/.stado/bin` also
    /// holds `SHA256SUMS` and release manifests.
    pub(super) async fn classify(&self, path: &str) -> Result<Classification, DeployError> {
        if let Some(decided) = classify_name(path) {
            return Ok(decided);
        }
        // The regular-file check and two-byte read share one single-line
        // command. The leading `f` preserves the distinction between a
        // non-file and a file whose `head` itself failed.
        let quoted = shlex_quote(path);
        let head = host_channel::run_command(
            self.target,
            &format!("test -f {quoted} && printf f && /usr/bin/head -c 2 {quoted}"),
            self.runner,
        )
        .await?;
        let Some(head) = head.stdout.strip_prefix('f') else {
            return Ok(Classification::Ignored);
        };
        if head == "#!" {
            return Ok(Classification::Script);
        }
        if !host_channel::remote_test(self.target, &format!("-x {quoted}"), self.runner).await? {
            return Ok(Classification::Ignored);
        }
        Ok(Classification::Program)
    }

    /// [`Self::classify`] for every entry directly under DIR, in one command.
    ///
    /// One round trip instead of one per file: `$HOME/.stado/bin` on
    /// charless-mac-mini holds 1408 retired helper scripts beside 41 programs,
    /// and reading each one's two bytes over its own channel round trip cost
    /// the first refresh through `release host-state` fourteen minutes. The
    /// readings are the same three — regular file, executable bit, first two
    /// bytes — taken by `find` on the host and printed one line per file as
    /// `<name>\t<x|->\t<hex>`, hex because two raw bytes of a Mach-O header
    /// can be anything, a newline included. Sorted by path so the report
    /// reads the same on every visit.
    pub(in crate::host_software::inspect) async fn classify_directory(
        &self,
        dir: &str,
    ) -> Result<Vec<(String, Classification)>, DeployError> {
        let listed = host_channel::run_command(
            self.target,
            &format!(
                "cd {} && /usr/bin/find -L . -maxdepth 1 -type f -exec /bin/sh -c \
                 '/usr/bin/printf \"%s\\t%s\\t\" \"$1\" \"$(test -x \"$1\" && printf x || printf -)\" \
                 && /usr/bin/head -c 2 \"$1\" | /usr/bin/od -An -tx1 | /usr/bin/tr -d \" \\n\"; \
                 /usr/bin/printf \"\\n\"' _ {{}} \\;",
                shlex_quote(dir)
            ),
            self.runner,
        )
        .await?;
        if !listed.ok() {
            return Err(DeployError(format!(
                "{}: could not list {dir}: {}",
                self.target.name,
                listed.stderr.trim()
            )));
        }
        let mut classified: Vec<(String, Classification)> = listed
            .stdout
            .lines()
            .filter_map(|line| classify_line(dir, line))
            .collect();
        classified.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(classified)
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_line, Classification};

    #[test]
    fn a_shebang_is_a_script_whether_or_not_it_is_executable() {
        assert_eq!(
            classify_line("/h/.stado/bin", "./helper.sh\t-\t2321"),
            Some((
                "/h/.stado/bin/helper.sh".to_string(),
                Classification::Script
            ))
        );
        assert_eq!(
            classify_line("/h/.stado/bin", "./helper\tx\t2321"),
            Some(("/h/.stado/bin/helper".to_string(), Classification::Script))
        );
    }

    #[test]
    fn an_executable_that_is_not_a_script_is_a_program() {
        assert_eq!(
            classify_line("/h/.stado/bin", "./stado\tx\tcffa"),
            Some(("/h/.stado/bin/stado".to_string(), Classification::Program))
        );
    }

    #[test]
    fn manifests_litter_and_rollback_copies_are_ignored() {
        let ignored = [
            "./SHA256SUMS\t-\t3030",
            "./.stado.release-version\tx\t302e",
            "./stado.previous\tx\tcffa",
        ];
        for line in ignored {
            assert_eq!(
                classify_line("/h/.stado/bin", line).map(|(_, decided)| decided),
                Some(Classification::Ignored),
                "{line}"
            );
        }
        assert_eq!(classify_line("/h/.stado/bin", "no tabs here"), None);
    }
}
