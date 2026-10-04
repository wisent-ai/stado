//! Explicit target-wide withdrawal, distinct from forgetting a declaration.
use crate::cli::{registry::commit_document, CmdError};
use crate::public_origin::{funnel, POLICY_KEY};
use serde_json::Value;

pub(crate) async fn withdraw(target_name: &str, json_output: bool) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(target_name).await?;
    let runner = crate::deploy::production_runner();
    let mut receipt = funnel::withdraw(&target, &runner)
        .await
        .map_err(CmdError::from)?;
    let generation = commit_document(|document| {
        let mut next = document.clone();
        let object = next.as_object_mut()
            .ok_or_else(|| CmdError::click("the registry document must be an object").stating(crate::primitives::failure::FailureCode::Config))?;
        if let Some(rows) = object.get_mut(POLICY_KEY) {
            let rows = rows.as_array_mut()
                .ok_or_else(|| CmdError::click("public_origins must be an array").stating(crate::primitives::failure::FailureCode::Config))?;
            rows.retain(|row| row["target"].as_str() != Some(target.name.as_str()));
            if rows.is_empty() { object.remove(POLICY_KEY); }
        }
        Ok(next)
    }).await.map_err(|error| {
        let mut wrapped = CmdError::click(format!(
            "public Funnel handlers on {} were withdrawn, but removing their declarations failed: {error}; withdrawal is not durably recorded",
            target.name
        ));
        wrapped.failure = error.failure;
        wrapped
    })?;
    receipt["schema"] = Value::String("stado.public-origin-withdrawal-receipt.v1".into());
    receipt["generation"] = serde_json::json!(generation);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        println!("withdrew public HTTPS Funnel access on {}; removed its public-origin declarations; generation {generation}", target.name);
    }
    Ok(())
}
