//! The read side: the vocabulary, the managed set, what the registry declares
//! about it, and the documents both are written in. No ssh lives here.

mod declaration;
mod document;
mod managed;
mod vocabulary;

pub use declaration::*;
pub use document::*;
pub use managed::*;
pub use vocabulary::*;
