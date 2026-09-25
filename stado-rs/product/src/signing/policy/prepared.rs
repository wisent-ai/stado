use super::{entitlements, Policy};
use crate::common::atomic_write;
use anyhow::{Context, Result};
use std::{
    borrow::Cow,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// Keeps the merged plist alive for the final signing pass and verification.
pub(crate) struct Prepared<'a> {
    policy: Cow<'a, Policy>,
    file: Option<PathBuf>,
}

impl Prepared<'_> {
    pub(crate) fn policy(&self) -> &Policy {
        &self.policy
    }

    fn close(&mut self) -> Result<()> {
        if let Some(path) = &self.file {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("remove signing policy {}", path.display()))
                }
            }
            self.file = None;
        }
        Ok(())
    }

    pub(crate) fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        match (result, self.close()) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Err(cleanup)) => {
                Err(error.context(format!("signing policy cleanup also failed: {cleanup:#}")))
            }
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        }
    }
}

impl Drop for Prepared<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("{error:#}");
        }
    }
}

impl Policy {
    pub(crate) fn prepare(&self, target: &Path) -> Result<Prepared<'_>> {
        if self.boolean_entitlements.is_empty() {
            return Ok(Prepared {
                policy: Cow::Borrowed(self),
                file: None,
            });
        }
        let mut expected = match &self.entitlements {
            Some((_, value)) => value.clone(),
            None => entitlements(target)?,
        };
        let dictionary = expected
            .as_dictionary_mut()
            .context("entitlement merge requires a dictionary")?;
        for (key, value) in &self.boolean_entitlements {
            dictionary.insert(key.clone(), plist::Value::Boolean(*value));
        }
        let parent = target.parent().context("signing target has no parent")?;
        let file = parent.join(format!(
            ".wisent-entitlements-{}.plist",
            uuid::Uuid::new_v4()
        ));
        let mut prepared = Prepared {
            policy: Cow::Borrowed(self),
            file: Some(file.clone()),
        };
        let mut bytes = Vec::new();
        expected.to_writer_xml(&mut bytes)?;
        atomic_write(&file, &bytes)?;
        let policy = Policy {
            entitlements: Some((file, expected)),
            hardened_runtime: self.hardened_runtime,
            boolean_entitlements: Default::default(),
        };
        prepared.policy = Cow::Owned(policy);
        Ok(prepared)
    }
}
