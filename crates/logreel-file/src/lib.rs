//! Append-only segment files for `logreel`.
//!
//! The binary layout is normative in `docs/SPEC.md`. This crate implements
//! one writer, day rotation with a byte cap, crash-safe recovery, probing and
//! a streaming reader. It never decodes payloads.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
