//! Library surface so `examples/` (and future tests) can reach the same modules
//! the `lucid` binary uses, instead of duplicating them.

pub mod cli;
pub mod config;
pub mod daemon;
pub mod harness;
pub mod pm;
pub mod presence;
pub mod state;
pub mod tracker;
pub mod worker;
