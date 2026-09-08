//! The object data plane: the coordinate a request addresses ([`query`]), the
//! read routes ([`read`]), the write routes ([`write`]), and the chunked
//! publication ([`compose`]).

mod compose;
mod query;
mod read;
mod write;

pub(crate) use query::{
    merged_object_metadata, object_from_query, object_list_from_query,
    public_release_object_from_query,
};
