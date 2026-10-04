use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::schema::Deployment;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub expected_registry_sha256: String,
    pub deployment: Deployment,
}

/// Why a saved inference plan could not be made, kept, read or consumed.
///
/// Each variant is one failure the caller tells apart: an unset `HOME` is
/// the environment's, an id this command never printed is the operator's
/// input, a file that is missing or unreadable is the filesystem's, and a
/// file that does not hold the plan its name promises is a damaged plan.
#[derive(Debug)]
pub enum PlanError {
    /// `HOME` is not set, so the plan directory is unknown.
    NoHome,
    /// The id is not one `stado inference plan` prints.
    InvalidId(String),
    /// The plan could not be encoded to compute its digest or body.
    Encode(serde_json::Error),
    /// The plan file could not be read.
    Read(PathBuf, std::io::Error),
    /// The plan file or its directory could not be written or removed.
    Write(PathBuf, std::io::Error),
    /// The plan file is not a plan.
    Invalid(PathBuf, serde_json::Error),
    /// The plan file holds a plan with another id.
    Mismatched(PathBuf),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHome => write!(
                f,
                "HOME is not set, so the inference plan directory is unknown"
            ),
            Self::InvalidId(id) => write!(
                f,
                "invalid inference plan id {id:?}; use the id `stado inference plan` printed"
            ),
            Self::Encode(error) => write!(f, "cannot encode the inference plan: {error}"),
            Self::Read(path, error) => {
                write!(f, "cannot read inference plan {}: {error}", path.display())
            }
            Self::Write(path, error) => {
                write!(f, "cannot write inference plan {}: {error}", path.display())
            }
            Self::Invalid(path, error) => {
                write!(f, "invalid inference plan {}: {error}", path.display())
            }
            Self::Mismatched(path) => write!(
                f,
                "inference plan {} holds a plan with another id than its file name",
                path.display()
            ),
        }
    }
}

impl std::error::Error for PlanError {}

pub fn document_digest(document: &Value) -> Result<String, PlanError> {
    let bytes = serde_json::to_vec(document).map_err(PlanError::Encode)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn plan_id(expected: &str, deployment: &Deployment) -> Result<String, PlanError> {
    let body = serde_json::to_vec(&(expected, deployment)).map_err(PlanError::Encode)?;
    Ok(format!("{:x}", Sha256::digest(body)))
}

fn root() -> Result<PathBuf, PlanError> {
    let home = std::env::var_os("HOME").ok_or(PlanError::NoHome)?;
    Ok(PathBuf::from(home).join(".stado").join("inference-plans"))
}

fn path(id: &str) -> Result<PathBuf, PlanError> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(PlanError::InvalidId(id.to_string()));
    }
    Ok(root()?.join(format!("{id}.json")))
}

pub fn create(document: &Value, deployment: Deployment) -> Result<Plan, PlanError> {
    let expected_registry_sha256 = document_digest(document)?;
    let id = plan_id(&expected_registry_sha256, &deployment)?;
    Ok(Plan {
        id,
        expected_registry_sha256,
        deployment,
    })
}

pub fn save(plan: &Plan) -> Result<PathBuf, PlanError> {
    let path = path(&plan.id)?;
    let directory = root()?;
    std::fs::create_dir_all(&directory).map_err(|error| PlanError::Write(directory, error))?;
    let temporary = path.with_extension("json.tmp");
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(plan).map_err(PlanError::Encode)?
    );
    std::fs::write(&temporary, body).map_err(|error| PlanError::Write(temporary.clone(), error))?;
    std::fs::rename(&temporary, &path).map_err(|error| PlanError::Write(path.clone(), error))?;
    Ok(path)
}

pub fn load(id: &str) -> Result<Plan, PlanError> {
    let path = path(id)?;
    let body =
        std::fs::read_to_string(&path).map_err(|error| PlanError::Read(path.clone(), error))?;
    let plan: Plan =
        serde_json::from_str(&body).map_err(|error| PlanError::Invalid(path.clone(), error))?;
    if plan.id != id {
        return Err(PlanError::Mismatched(path));
    }
    Ok(plan)
}

pub fn consume(id: &str) -> Result<(), PlanError> {
    let path = path(id)?;
    std::fs::remove_file(&path).map_err(|error| PlanError::Write(path, error))
}
