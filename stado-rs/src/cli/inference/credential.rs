use serde_json::json;
use uuid::Uuid;

use crate::cli::CmdError;
use crate::inference::schema::LOCAL_PROVIDER_CREDENTIAL;

pub async fn init(json_output: bool) -> Result<(), CmdError> {
    let vault = crate::skarbiec::Client::configured()
        .map_err(|error| CmdError::click(error.to_string()))?;
    let existing = vault
        .list_items()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .into_iter()
        .any(|item| item.id == LOCAL_PROVIDER_CREDENTIAL);
    if existing {
        return Err(CmdError::click(format!(
            "credential {LOCAL_PROVIDER_CREDENTIAL:?} already exists; refusing unsafe implicit rotation"
        )));
    }
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    vault
        .write_item(
            LOCAL_PROVIDER_CREDENTIAL,
            "stado-secret",
            &json!({"token": token}),
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "created",
                "item": LOCAL_PROVIDER_CREDENTIAL,
            }))?
        );
    } else {
        println!("created inference credential {LOCAL_PROVIDER_CREDENTIAL:?}");
    }
    Ok(())
}

pub async fn read() -> Result<String, CmdError> {
    let vault = crate::skarbiec::Client::configured()
        .map_err(|error| CmdError::click(error.to_string()))?;
    vault
        .read_string(LOCAL_PROVIDER_CREDENTIAL, "token")
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {LOCAL_PROVIDER_CREDENTIAL:?} has no non-empty string field \"token\""
            ))
        })
}
