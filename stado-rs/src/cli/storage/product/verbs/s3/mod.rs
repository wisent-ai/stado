//! The S3 face of a product namespace: the presigned URL of one object
//! (`stado storage url --presign`), the endpoint, bucket and access key any
//! S3 client reaches a namespace through, confronted with one signed request
//! before it is reported (`stado storage s3-endpoint`), and a copy or move of
//! one object on the server (`stado storage cp|mv`, [`copy`]).
//!
//! All sign with the bearer this client presents for the object API
//! (`session::bearer`): it is the namespace's S3 secret access key, and the
//! namespace is its access key id.

pub(in crate::cli::storage) mod copy;

use crate::cli::storage::*;
use crate::remote::sigv4::{self, Credentials, S3_ROOT, STADO_REGION};

use super::super::api::session::bearer::namespace_bearer;

/// One request with an empty body to this client's S3 endpoint at `url`,
/// signed as `namespace` with this client's object API bearer, carrying and
/// signing `carried`.
async fn signed(
    method: reqwest::Method,
    url: &url::Url,
    namespace: &str,
    carried: &[(&str, &str)],
) -> Result<reqwest::Response, CmdError> {
    let secret = namespace_bearer()?;
    let credentials = Credentials {
        access_key_id: namespace,
        secret_access_key: &secret,
    };
    let now = chrono::Utc::now();
    let headers = sigv4::sign_empty_request(
        method.as_str(),
        url,
        &credentials,
        STADO_REGION,
        now,
        carried,
    )
    .map_err(CmdError::usage)?;
    let mut request = fleet_https_client()?.request(method, url.clone());
    for (name, value) in headers {
        request = request.header(name, value);
    }
    for (name, value) in carried {
        request = request.header(*name, *value);
    }
    Ok(sent(request).await?)
}

/// The S3 endpoint's refusal of `what`, in its own code and message, stated
/// as an authorization, infrastructure or configuration failure by status.
fn refused_by(endpoint: &str, what: &str, response: &reqwest::Response) -> CmdError {
    let status = response.status();
    let header = |name: &str| match response.headers().get(name) {
        Some(value) => String::from_utf8_lossy(value.as_bytes()).into_owned(),
        None => String::new(),
    };
    let code = if status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::UNAUTHORIZED
    {
        crate::primitives::failure::FailureCode::Auth
    } else if status.is_server_error() {
        crate::primitives::failure::FailureCode::InfraDown
    } else {
        crate::primitives::failure::FailureCode::Config
    };
    CmdError::click(format!(
        "the S3 endpoint {endpoint} refused {what} with HTTP {}: {} {}",
        status.as_u16(),
        header("x-amz-error-code"),
        header("x-amz-error-message")
    ))
    .stating(code)
}

/// The method a presigned URL admits.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum PresignMethod {
    /// Download the object.
    Get,
    /// Upload the object's bytes.
    Put,
}

impl PresignMethod {
    fn http(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
        }
    }
}

/// The object API origin this client reaches, required for anything S3.
fn object_api_origin(what: &str) -> Result<url::Url, CmdError> {
    RemoteObjectApi::endpoint_from_env_or_config()?.ok_or_else(|| {
        CmdError::click(format!(
            "STADO_API_URL or storage.stado.url must name the object API to {what}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })
}

/// The S3 endpoint's root path on `origin`.
fn s3_root(origin: &url::Url) -> String {
    format!("{}{S3_ROOT}", origin.path().trim_end_matches('/'))
}

/// A presigned URL for one product object, valid for `expires` seconds.
pub(in crate::cli::storage) fn presigned_url(
    object: &crate::remote::object_store::ObjectRef,
    method: PresignMethod,
    expires: u64,
) -> Result<url::Url, CmdError> {
    if object.release_namespace().is_some() {
        return Err(CmdError::usage(
            "release objects are read from their public URL without a signature; run `stado \
             storage url` without --presign",
        ));
    }
    let origin = object_api_origin("presign an object URL")?;
    let secret = namespace_bearer()?;
    let path = sigv4::object_path(&s3_root(&origin), object.namespace(), object.key());
    sigv4::presign(
        method.http(),
        &origin,
        &path,
        &Credentials {
            access_key_id: object.namespace(),
            secret_access_key: &secret,
        },
        STADO_REGION,
        chrono::Utc::now(),
        expires,
    )
    .map_err(CmdError::usage)
}

#[derive(Args, Debug)]
pub struct StorageS3EndpointArgs {
    /// The namespace: the bucket and the access key id an S3 client uses.
    namespace: String,
    #[arg(long)]
    json: bool,
}

/// `stado storage s3-endpoint`: report how an S3 client reaches one
/// namespace, after one signed HeadBucket proves the endpoint accepts this
/// client's secret for it.
pub(in crate::cli::storage) async fn s3_endpoint(
    args: &StorageS3EndpointArgs,
) -> Result<(), CmdError> {
    let sentinel = crate::remote::object_store::ObjectRef::new(&args.namespace, "sentinel")
        .map_err(|error| CmdError::usage(error.to_string()))?;
    let namespace = sentinel.namespace().to_string();
    if sentinel.release_namespace().is_some() {
        return Err(CmdError::usage(format!(
            "{namespace} is a release namespace, published through `stado storage put` and \
             read from its public URL; it has no S3 bucket"
        )));
    }
    let origin = object_api_origin("describe an S3 endpoint")?;
    let root = s3_root(&origin);
    let mut endpoint = origin.clone();
    endpoint.set_path(&root);
    let mut bucket_url = origin.clone();
    bucket_url.set_path(&sigv4::object_path(&root, &namespace, ""));
    let response = signed(reqwest::Method::HEAD, &bucket_url, &namespace, &[]).await?;
    let status = response.status();
    if !status.is_success() {
        let what = format!("a signed HeadBucket of {namespace}");
        return Err(refused_by(endpoint.as_str(), &what, &response));
    }
    let item = match crate::config::object_api_namespace(&namespace) {
        Some(policy) => Value::String(policy.item().to_string()),
        None => Value::Null,
    };
    let report = json!({
        "namespace": namespace,
        "endpoint": endpoint.as_str(),
        "bucket": namespace,
        "access_key_id": namespace,
        "region": STADO_REGION,
        "addressing": "path",
        "secret_access_key": {
            "source": "the object API bearer this client presents: STADO_API_TOKEN, \
                       STADO_API_TOKEN_FILE or storage.stado.token_file",
            "item": item,
            "field": "token",
        },
        "check": {
            "request": format!("HEAD {}", bucket_url.path()),
            "status": status.as_u16(),
        },
    });
    if args.json {
        echo_json(&report)?;
    } else {
        println!("endpoint          {endpoint}");
        println!("bucket            {namespace}");
        println!("access key id     {namespace}");
        println!(
            "secret access key the object API bearer of {namespace} \
             (STADO_API_TOKEN, STADO_API_TOKEN_FILE or storage.stado.token_file)"
        );
        println!("region            {STADO_REGION}");
        println!("addressing        path");
        println!(
            "checked           HEAD {} answered {}",
            bucket_url.path(),
            status.as_u16()
        );
    }
    Ok(())
}
