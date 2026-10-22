//! The signer's side: a presigned URL, and the headers of one signed request
//! with no body. Both build the canonical request from the same pieces the
//! endpoint rebuilds it from (`super`).

use chrono::{DateTime, Utc};

use super::{
    canonical_header_value, canonical_query, canonical_request, hex_sha256, signature, signing_key,
    string_to_sign, uri_encode, Scope, ALGORITHM, AMZ_DATE_FORMAT, MAX_PRESIGNED_SECONDS,
    UNSIGNED_PAYLOAD,
};

/// The access key and secret one signer holds.
pub struct Credentials<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
}

/// The `host` header value a URL is requested under: its host, and its port
/// when it is not the scheme's default.
pub fn host_header(url: &url::Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// The path of one object on an S3 endpoint whose base is `base_path`
/// (`/s3` on Stado's object API): the bucket and each key segment
/// SigV4-encoded, which is the exact path a signer puts on the wire. An
/// empty key addresses the bucket itself.
pub fn object_path(base_path: &str, bucket: &str, key: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if key.is_empty() {
        format!("{base}/{}", uri_encode(bucket, false))
    } else {
        format!(
            "{base}/{}/{}",
            uri_encode(bucket, false),
            uri_encode(key, true)
        )
    }
}

/// A presigned URL for `method` on `path` of `origin`, valid for `expires`
/// seconds from `now`. Only `host` is signed and the payload is unsigned, so
/// whoever holds the URL may send it, with any body, until it expires.
pub fn presign(
    method: &str,
    origin: &url::Url,
    path: &str,
    credentials: &Credentials<'_>,
    region: &str,
    now: DateTime<Utc>,
    expires: u64,
) -> Result<url::Url, String> {
    if std::num::NonZeroU64::new(expires).is_none() || expires > MAX_PRESIGNED_SECONDS {
        return Err(format!(
            "a presigned URL lives between 1 and {MAX_PRESIGNED_SECONDS} seconds; {expires} was asked"
        ));
    }
    let mut url = origin.clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    let host = host_header(&url).ok_or_else(|| format!("{origin} names no host"))?;
    let scope = Scope::s3(now, region);
    let amz_date = now.format(AMZ_DATE_FORMAT).to_string();
    let pairs = vec![
        ("X-Amz-Algorithm".to_string(), ALGORITHM.to_string()),
        (
            "X-Amz-Credential".to_string(),
            format!("{}/{}", credentials.access_key_id, scope.render()),
        ),
        ("X-Amz-Date".to_string(), amz_date.clone()),
        ("X-Amz-Expires".to_string(), expires.to_string()),
        ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
    ];
    let query = canonical_query(&pairs);
    let request = canonical_request(
        method,
        path,
        &query,
        &[("host".to_string(), host)],
        "host",
        UNSIGNED_PAYLOAD,
    );
    let key = signing_key(credentials.secret_access_key, &scope);
    let signed = signature(&key, &string_to_sign(&amz_date, &scope, &request));
    url.set_query(Some(&format!("{query}&X-Amz-Signature={signed}")));
    Ok(url)
}

/// The `authorization`, `x-amz-date` and `x-amz-content-sha256` headers of a
/// header-signed request with an empty body. `carried` are the other headers
/// the request carries (`x-amz-copy-source` for a CopyObject), lowercase;
/// they are signed too, so the endpoint refuses a request whose value was
/// changed on the way.
pub fn sign_empty_request(
    method: &str,
    url: &url::Url,
    credentials: &Credentials<'_>,
    region: &str,
    now: DateTime<Utc>,
    carried: &[(&str, &str)],
) -> Result<Vec<(&'static str, String)>, String> {
    let host = host_header(url).ok_or_else(|| format!("{url} names no host"))?;
    let scope = Scope::s3(now, region);
    let amz_date = now.format(AMZ_DATE_FORMAT).to_string();
    let payload = hex_sha256(b"");
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    let mut headers = vec![
        ("host".to_string(), host),
        ("x-amz-content-sha256".to_string(), payload.clone()),
        ("x-amz-date".to_string(), amz_date.clone()),
    ];
    headers.extend(
        carried
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), canonical_header_value(&[value]))),
    );
    // SigV4 orders the canonical headers by name.
    headers.sort();
    let names: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    let signed_headers = names.join(";");
    let request = canonical_request(
        method,
        url.path(),
        &canonical_query(&pairs),
        &headers,
        &signed_headers,
        &payload,
    );
    let key = signing_key(credentials.secret_access_key, &scope);
    let signed = signature(&key, &string_to_sign(&amz_date, &scope, &request));
    Ok(vec![
        (
            "authorization",
            format!(
                "{ALGORITHM} Credential={}/{}, SignedHeaders={signed_headers}, Signature={signed}",
                credentials.access_key_id,
                scope.render()
            ),
        ),
        ("x-amz-date", amz_date),
        ("x-amz-content-sha256", payload),
    ])
}
