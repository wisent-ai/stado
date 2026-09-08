//! Where the EC2 client's credentials come from: the coordinator's scoped
//! `stado-aws` Skarbiec item, or — on an adapter host without that grant —
//! the host's own IMDSv2 workload identity. Process-environment and
//! shared-profile chains stay deliberately bypassed.

/// One `stado-aws` field, tried under each accepted name.
///
/// The broker requires a named field on `/v1/items/read`; a whole-item read
/// answers `HTTP 400 {"error":"field required"}`. That refusal used to reach
/// the operator as an AWS-credential failure while the item was readable.
///
/// A refusal on one candidate name does not end the search: the grant may
/// name `aws_access_key_id` where the first attempt asked for
/// `access_key_id`. The last refusal is returned only when no name resolved,
/// so an unauthorized grant still surfaces its own error rather than a
/// misleading "missing value".
async fn stado_aws_field(names: &[&str]) -> Result<Option<String>, crate::skarbiec::SkarbiecError> {
    let mut refusal = None;
    for name in names {
        match crate::skarbiec::read_string("stado-aws", name).await {
            Ok(Some(value)) if !value.trim().is_empty() => return Ok(Some(value)),
            Ok(_) => {}
            Err(error) => refusal = Some(error),
        }
    }
    match refusal {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

/// AWS SDK configuration from either the coordinator's scoped `stado-aws`
/// Skarbiec item or, on adapter hosts without a grant, the host's IMDSv2
/// workload identity. Process-environment and shared-profile credential
/// chains are deliberately bypassed.
pub(crate) async fn sdk_config(
    region: &str,
) -> Result<aws_config::SdkConfig, crate::skarbiec::SkarbiecError> {
    let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
    if !region.is_empty() {
        loader = loader.region(aws_config::Region::new(region.to_string()));
    }
    if !crate::config::skarbiec_consumer().trim().is_empty()
        && !crate::config::skarbiec_token_file().trim().is_empty()
    {
        let access_key = stado_aws_field(&["access_key_id", "aws_access_key_id"])
            .await?
            .ok_or_else(|| {
                crate::skarbiec::SkarbiecError::MissingValue("stado-aws.access_key_id".to_string())
            })?;
        let secret_key = stado_aws_field(&["secret_access_key", "aws_secret_access_key"])
            .await?
            .ok_or_else(|| {
                crate::skarbiec::SkarbiecError::MissingValue(
                    "stado-aws.secret_access_key".to_string(),
                )
            })?;
        let session_token = stado_aws_field(&["session_token", "aws_session_token"]).await?;
        let credentials = aws_sdk_ec2::config::Credentials::new(
            access_key,
            secret_key,
            session_token,
            None,
            "Skarbiec",
        );
        loader = loader.credentials_provider(credentials);
    } else {
        let imds = aws_config::imds::credentials::ImdsCredentialsProvider::builder().build();
        aws_sdk_ec2::config::ProvideCredentials::provide_credentials(&imds)
            .await
            .map_err(|error| {
                crate::skarbiec::SkarbiecError::Deployment(format!(
                    "AWS adapter IMDSv2 identity is unavailable: {error}"
                ))
            })?;
        loader = loader.credentials_provider(imds);
    }
    Ok(loader.load().await)
}
