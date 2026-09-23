//! Guile language server library: document tracking, the tolerant reader,
//! syntax-only editor features, and the Guile worker bridge.

pub mod adapters;
pub mod cache;
pub mod cli;
pub mod completion;
pub mod config;
pub mod diagnostics;
pub mod documents;
pub mod features;
pub mod format;
pub mod imports;
pub mod resolver;
pub mod roots;
pub mod scope;
pub mod semantic;
pub mod syntax;
pub mod worker;
