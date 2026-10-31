//! Core event pipeline for `logreel`.
//!
//! The crate owns the event contract, the record envelope, filters, the
//! writer pipeline, text projection and scopes. It performs no I/O itself:
//! encoded batches leave through a sink supplied by the caller.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
