//! Storage contract for `logreel`.
//!
//! This crate is deliberately dependency free: it defines the encoded batch,
//! the durability policy, the `Store` trait and the error taxonomy shared by
//! every backend.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
