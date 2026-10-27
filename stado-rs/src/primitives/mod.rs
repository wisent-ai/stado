//! The pieces every other module in this crate stands on: the declared
//! constants, the environment's declarations, the failure type each command
//! returns, the process helpers a command runs work with, the reader for a
//! refused HTTP answer's body, the removal of a staging file a failed write
//! left, the JSON documents Stado keeps in files of its own, the serde word
//! a closed vocabulary prints, the lock a panicked holder left, and the
//! fixtures a test builds against.

pub mod archive;
pub mod constants;
pub mod digest;
pub mod document;
pub mod environment;
pub mod failure;
pub mod file_mode;
pub mod http;
pub mod lock;
pub mod model_judgement;
pub mod presence;
pub mod procutil;
pub mod removal;
pub mod samples;
pub mod staging;
pub mod tally;
pub mod testutil;
pub mod tool_path;
pub mod word;
