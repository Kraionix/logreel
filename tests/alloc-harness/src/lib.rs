//! Allocation harness for the logreel emit path.
//!
//! This package deliberately does not inherit the workspace lints: it needs
//! a global allocator, which is unsafe code in the test harness only.
