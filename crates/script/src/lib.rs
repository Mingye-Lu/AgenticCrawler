//! Autonomous Script Protocol — parser and AST for acrawl scripts.
//!
//! This crate provides parsing and AST representation for acrawl's script language.
//! It is parser/AST-only with no runtime, browser, or tokio dependencies.
//! The executor lives in `crates/agent/src/script_executor/`.

pub mod error;
pub mod grammar;
pub mod parser;
pub mod persistence;
