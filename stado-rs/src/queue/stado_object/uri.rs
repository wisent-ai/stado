//! The URI shape every route is addressed through.
//!
//! One namespace-qualified [`ObjectRef`] per path, one origin-relative
//! endpoint per route, and one authenticated request builder. Every read,
//! write, stat and listing in this module goes out through these.

use reqwest::{Method, Url};

use crate::remote::object_store::ObjectRef;
use crate::queue::StorageError;

use super::StadoObjectBackend;

impl StadoObjectBackend {
    pub(super) fn object(&self, path: &str) -> Result<ObjectRef, StorageError> {
        ObjectRef::new(&self.namespace, path)
    }

    pub(super) fn url(&self, endpoint: &str) -> Url {
        let mut url = self.base_url.clone();
        url.set_path(endpoint);
        url
    }

    pub(super) fn object_url(
        &self,
        path: &str,
        options: &[(&str, &str)],
    ) -> Result<Url, StorageError> {
        let object = self.object(path)?;
        let mut url = self.url("/api/object");
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("uri", &object.to_string());
            for (name, value) in options {
                query.append_pair(name, value);
            }
        }
        Ok(url)
    }

    pub(super) fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
    }

    /// The listing URL for `prefix`, with the prefix validation every listing
    /// route needs: the gateway takes a namespace and a prefix and nothing
    /// else, so both listings address exactly the same endpoint.
    pub(super) fn list_url(&self, prefix: &str) -> Result<Url, StorageError> {
        ObjectRef::new(&self.namespace, &format!("{prefix}sentinel"))?;
        let mut url = self.url("/api/object/list");
        url.query_pairs_mut()
            .append_pair("namespace", &self.namespace)
            .append_pair("prefix", prefix);
        Ok(url)
    }
}
