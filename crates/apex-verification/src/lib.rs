//! Verification: outcome contracts, observable checks and repair guidance.

pub mod checks;
pub mod contract;

pub use checks::{repair_instruction, verify, VerificationReport};
pub use contract::OutcomeContract;
