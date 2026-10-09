//! Waits on an HTTP answer or an AWS SDK call.

use std::future::Future;

use crate::{until, Kind};

/// Send one HTTP request and wait for its answer to begin, saying so at
/// both ends. What is waited for is the request's method, path and decoded
/// query (an object reads as its `stado://` URI, not its percent-encoding);
/// where is `service` at the request's origin.
pub async fn send(
    kind: Kind,
    service: &str,
    builder: reqwest::RequestBuilder,
) -> reqwest::Result<reqwest::Response> {
    let (client, request) = builder.build_split();
    send_built(kind, service, client, request?).await
}

/// [`send`] for a request already built: a caller that must read where the
/// request goes before sending it — to refuse a route this host's resolver
/// reports as held — builds it, reads its URL, and hands it here.
pub async fn send_built(
    kind: Kind,
    service: &str,
    client: reqwest::Client,
    request: reqwest::Request,
) -> reqwest::Result<reqwest::Response> {
    let url = request.url();
    let query: Vec<String> = url
        .query_pairs()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let what = format!("{} {} {}", request.method(), url.path(), query.join(" "));
    let place = format!("{service} {}", url.origin().ascii_serialization());
    until(kind, what, place, client.execute(request)).await
}

/// Send one HTTP request to any other service and wait for its answer to
/// begin, saying so at both ends. What is waited for is the method and the
/// path — never the query, which may carry a credential a service takes
/// there; where is the request's origin.
pub async fn request(builder: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    let (client, request) = builder.build_split();
    let request = request?;
    let what = format!("{} {}", request.method(), request.url().path());
    let place = request.url().origin().ascii_serialization();
    until(Kind::Network, what, place, client.execute(request)).await
}

/// [`request`] for a blocking client: send one HTTP request and wait on this
/// thread for its answer to begin, saying so at both ends, naming the method
/// and path only.
pub fn request_blocking(
    builder: reqwest::blocking::RequestBuilder,
) -> reqwest::Result<reqwest::blocking::Response> {
    let (client, request) = builder.build_split();
    let request = request?;
    let what = format!("{} {}", request.method(), request.url().path());
    let place = request.url().origin().ascii_serialization();
    crate::blocking(Kind::Network, what, place, || client.execute(request))
}

/// Wait for one AWS SDK call — the future its fluent builder's `send`
/// returns — saying so at both ends. What is waited for is the operation's
/// builder as the SDK names it (`aws_sdk_s3::operation::get_object::…`).
pub async fn sdk<T, E, F>(call: F) -> Result<T, E>
where
    E: std::error::Error,
    F: Future<Output = Result<T, E>>,
{
    let what = std::any::type_name::<F>().trim_end_matches("::send::{{closure}}");
    until(Kind::Network, what, "the AWS API", call).await
}
