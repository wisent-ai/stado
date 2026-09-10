//! The pieces every other module in this crate stands on: the declared
//! constants, the failure type each command returns, the process helpers a
//! command runs work with, and the fixtures a test builds against.

pub mod constants;
pub mod failure;
pub mod procutil;
pub mod testutil;
