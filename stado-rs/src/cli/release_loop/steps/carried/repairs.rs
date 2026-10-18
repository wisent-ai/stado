//! Every open defect's recorded repair, as Oko's register holds it: the one
//! reader the release loop, its plan and the delivery routine share.

use serde::Deserialize;

use super::super::process::run;
use super::OKO;
use crate::cli::CmdError;

#[derive(Deserialize)]
struct Defect {
    id: String,
    #[serde(default)]
    repaired_in: Option<String>,
    #[serde(default)]
    owner: Option<String>,
}

/// One open defect's recorded repair: the defect, the session that owns it
/// (none when nobody does), the repository the repair was committed in and
/// the repair commit.
pub struct Repair {
    pub(crate) id: String,
    pub(crate) owner: Option<String>,
    pub(crate) repository: String,
    pub(crate) commit: String,
}

/// Every open defect's recorded repair, read once for a whole pass: the
/// register holds thousands of defects, and the workspace hundreds of
/// checkouts. A defect with no recorded repair is not one. A recorded repair
/// that names no `<repository>@<commit>` is refused with every such defect
/// and its record: dropped, its defect stayed open with no release ever
/// carrying it and nothing saying why.
pub async fn open_repairs() -> Result<Vec<Repair>, CmdError> {
    let listed = run(OKO, &["defects", "list", "--open", "--json"], None).await?;
    let defects: Vec<Defect> = serde_json::from_str(&listed).map_err(|error| {
        CmdError::click(format!(
            "oko defects list --open --json answered no defect list: {error}"
        ))
    })?;
    let mut repairs = Vec::new();
    let mut unnamed = Vec::new();
    for defect in defects {
        let Some(revision) = defect.repaired_in else {
            continue;
        };
        match revision.rsplit_once('@') {
            Some((repository, commit)) if !repository.is_empty() && !commit.is_empty() => repairs
                .push(Repair {
                    id: defect.id,
                    owner: defect.owner,
                    repository: repository.to_string(),
                    commit: commit.to_string(),
                }),
            _ => unnamed.push(format!("{} ({revision:?})", defect.id)),
        }
    }
    if !unnamed.is_empty() {
        return Err(CmdError::click(format!(
            "Oko records repairs that name no <repository>@<commit>, so no commit can be found \
             to carry them: {}; record each with `oko defects repaired ID --revision \
             REPOSITORY@COMMIT`",
            unnamed.join(", ")
        )));
    }
    Ok(repairs)
}
