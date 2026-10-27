//! The S3 API's wire on this listener: what one request addresses
//! ([`request`]), the body a signed write carries and the digests that
//! prove it ([`payload`], [`checksum`]), and the XML documents and error
//! shape every answer is ([`xml`]).

mod checksum;
mod payload;
mod request;
mod xml;

pub(crate) use payload::body;
pub(crate) use request::{
    allow, invalid, method, not_implemented, object, path_decode, storage, Query,
};
pub(crate) use xml::{document, escape, read, CompleteDocument, DeleteDocument, S3Error, XML};
