//! Conservative duplicate matching.
//!
//! The same property is often advertised several times (on several platforms, by several
//! agents, relisted after an ad expired). Folding such ads together makes search results honest,
//! but a **wrong** merge hides a different property behind another one, so every choice here
//! favours keeping listings apart:
//!
//! * a perceptual-hash match only *proposes* a candidate; it is never proof of identity;
//! * similar price, neighbourhood, area or image alone can never establish identity;
//! * missing and contradictory evidence is kept in the stored evidence, never dropped;
//! * automatic merging does not exist: the rule below has not been evaluated against real
//!   advertisements, so every merge is a user decision (reversible, audited).
//!
//! The thresholds in [`compare::MatchConfig`] are experimental configuration values.

pub mod candidates;
pub mod compare;
pub mod evaluation;
pub mod load;
pub mod run;
#[cfg(test)]
pub(crate) mod testkit;

pub use compare::{Comparison, MatchConfig, MatchInput, compare};

/// Version of the comparison rule. Stored with every decision; bump it whenever the rule or its
/// thresholds change so old decisions stay interpretable.
pub const RULE_VERSION: &str = "match_rules/1";
