//! One dispatch per declaration block: the match arms of the single
//! `stado service` dispatch, grouped exactly as [`super::spec`] groups the
//! variants they answer.

use super::*;

pub(super) mod environment;
pub(super) mod lifecycle;
pub(super) mod read;
pub(super) mod runtime;
