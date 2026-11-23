//! The `sruman` binary's modules: CLI parsing, `config.toml`, settings,
//! logging, output and the commands. A library target so integration tests
//! can reach them.

#![forbid(unsafe_code)]

pub mod app;
pub mod cli;
pub mod config;
pub mod logger;
pub mod output;
pub mod settings;
