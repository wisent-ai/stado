//! The credential fields `sandbox fleet exec --credential NAME=ITEM#FIELD`
//! hands to the command inside the sandbox, read exactly as `stado
//! credentials run --env` reads them: from the selected store, or with
//! `--route` under one consumer's own grant. Each value travels in the host
//! program on standard input, is exported in the host shell and handed to
//! `docker exec -e NAME` by name, so it is in no argv, file, receipt or
//! Stado output, and only the sandboxed command's environment holds it.

use clap::Args;

use crate::cli::CmdError;

#[derive(Args)]
pub struct Credentials {
    /// One variable and the credential field it takes; repeat for each.
    #[arg(long = "credential", value_name = "NAME=ITEM#FIELD")]
    credential: Vec<String>,
    /// Skarbiec route for reads under one consumer's grant. Requires
    /// --consumer and --grant-file; never uses the store administrator.
    #[arg(long, requires_all = ["consumer", "grant_file"])]
    route: Option<String>,
    /// Identity granted these fields.
    #[arg(long, requires = "route")]
    consumer: Option<String>,
    /// File holding that consumer's grant.
    #[arg(long, requires = "route")]
    grant_file: Option<String>,
}

impl Credentials {
    /// Each named variable with its granted value; nothing is read when no
    /// credential is named.
    pub(super) async fn values(self) -> Result<Vec<(String, String)>, CmdError> {
        if self.credential.is_empty() {
            return Ok(Vec::new());
        }
        let values = crate::cli::secrets::granted(
            &self.credential,
            self.route,
            self.consumer,
            self.grant_file,
        )
        .await?;
        Ok(values.into_iter().collect())
    }
}
