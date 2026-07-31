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

pub fn document_digest(document: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(document).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn plan_id(expected: &str, deployment: &Deployment) -> Result<String, String> {
    let body = serde_json::to_vec(&(expected, deployment)).map_err(|error| error.to_string())?;
    let digest = format!("{:x}", Sha256::digest(body));
    let width = Sha256::output_size() / (u8::BITS as usize);
    Ok(digest.chars().take(width).collect())
}

fn root() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".stado").join("inference-plans"))
}

fn path(id: &str) -> Result<PathBuf, String> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err("invalid inference plan id".to_string());
    }
    Ok(root()?.join(format!("{id}.json")))
}

pub fn create(document: &Value, deployment: Deployment) -> Result<Plan, String> {
    let expected_registry_sha256 = document_digest(document)?;
    let id = plan_id(&expected_registry_sha256, &deployment)?;
    Ok(Plan {
        id,
        expected_registry_sha256,
        deployment,
    })
}

pub fn save(plan: &Plan) -> Result<PathBuf, String> {
    let path = path(&plan.id)?;
    let parent = path
        .parent()
        .ok_or_else(|| "inference plan path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let body = format!(
        "{}\n",
        serde_json::to_string_pretty(plan).map_err(|error| error.to_string())?
    );
    std::fs::write(&temporary, body).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
    Ok(path)
}

pub fn load(id: &str) -> Result<Plan, String> {
    let path = path(id)?;
    let body = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read inference plan {}: {error}", path.display()))?;
    let plan: Plan = serde_json::from_str(&body)
        .map_err(|error| format!("invalid inference plan {}: {error}", path.display()))?;
    if plan.id != id {
        return Err("inference plan id does not match its file name".to_string());
    }
    Ok(plan)
}
pub fn list() -> Result<Vec<Plan>, String> {
    let directory = root()?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut plans = std::fs::read_dir(&directory)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .map(|entry| {
            let path = entry.path();
            let body = std::fs::read_to_string(&path).map_err(|error| {
                format!("cannot read inference plan {}: {error}", path.display())
            })?;
            serde_json::from_str(&body)
                .map_err(|error| format!("invalid inference plan {}: {error}", path.display()))
        })
        .collect::<Result<Vec<Plan>, String>>()?;
    plans.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(plans)
}

pub fn consume(id: &str) -> Result<(), String> {
    let path = path(id)?;
    std::fs::remove_file(path).map_err(|error| error.to_string())
}
