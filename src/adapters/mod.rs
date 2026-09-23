//! Library-specific integrations behind one boundary. The core never names
//! Guix constructors; adapters recognise library forms by what the worker
//! resolved them to, and every adapter failure degrades to the generic
//! behaviour.

pub mod guix;
mod guix_indent;
pub mod validate;
