//! The pieces every other module in this crate stands on: the declared
//! constants, the failure type each command returns, the process helpers a
//! command runs work with, the reader for a refused HTTP answer's body, the
//! removal of a staging file a failed write left, and the fixtures a test
//! builds against.

pub mod constants;
pub mod failure;
pub mod file_mode;
pub mod http;
pub mod procutil;
pub mod staging;
pub mod testutil;
