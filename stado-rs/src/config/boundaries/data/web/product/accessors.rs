//! Read accessors of one declared web product.

use std::collections::BTreeMap;

use super::{WebApiDatabase, WebApiProduct};

impl WebApiProduct {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    pub fn consumer(&self) -> &str {
        &self.consumer
    }

    pub fn readyz(&self) -> &str {
        &self.readyz
    }

    pub fn edge(&self) -> &str {
        &self.edge
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    /// Variable name to `item#field`, the same spelling
    /// `.wisent-release.json` uses for a build secret.
    pub fn secrets(&self) -> &BTreeMap<String, String> {
        &self.secrets
    }

    pub fn database(&self) -> Option<&WebApiDatabase> {
        self.database.as_ref()
    }

    /// Where this hostname redirects, for a product that is a redirect and
    /// nothing else.
    pub fn redirect_to(&self) -> Option<&str> {
        self.redirect_to.as_deref()
    }

    /// The registry service this hostname is published in front of.
    pub fn upstream_service(&self) -> Option<&str> {
        self.upstream_service.as_deref()
    }

    /// Whether this declaration describes a unit `stado web` owns.
    ///
    /// A redirect lives entirely in the edge's configuration; a hostname in
    /// front of an existing service belongs to whoever declared that service.
    /// Neither has a web release, so `deploy`, the release pipeline and the
    /// unit half of `status` have nothing to do with either.
    pub fn owns_a_unit(&self) -> bool {
        self.redirect_to.is_none() && self.upstream_service.is_none()
    }

    /// The path prefix this product is mounted at, for a product that lives
    /// under another declaration's hostname.
    pub fn path_prefix(&self) -> Option<&str> {
        self.path_prefix.as_deref()
    }

    /// Whether this product owns its hostname. A mount does not: the owner's
    /// declaration holds the record, the certificate and the catch-all.
    pub fn owns_its_hostname(&self) -> bool {
        self.path_prefix.is_none()
    }

    pub fn is_redirect(&self) -> bool {
        self.redirect_to.is_some()
    }
}

impl WebApiDatabase {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn field(&self) -> &str {
        &self.field
    }

    pub fn variable(&self) -> &str {
        &self.variable
    }
}
