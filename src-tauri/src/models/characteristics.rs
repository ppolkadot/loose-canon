use serde::{Deserialize, Serialize};

use super::{
    area::{BuiltArea, LotArea, UnspecifiedArea, UsableArea},
    confidence::Confidence,
    evidence::{Evidence, ParseWarning},
};

/// A value pulled out of text together with how sure we are and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extracted<T> {
    pub value: T,
    pub confidence: Confidence,
    pub evidence: Evidence,
    pub warnings: Vec<ParseWarning>,
}

/// What we know about one characteristic of a property.
///
/// Three states on purpose: a missing value is `Unknown` (never `0`), and when sources
/// disagree the disagreement is kept as `Conflicting` instead of silently picking a winner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum Fact<T> {
    Unknown,
    Known(Extracted<T>),
    /// Two or more *distinct* values were found; each keeps its own evidence.
    Conflicting(Vec<Extracted<T>>),
}

// Written by hand on purpose: `#[derive(Default)]` would require `T: Default`, which types such
// as `LotArea` (no meaningful zero) deliberately do not implement.
#[allow(clippy::derivable_impls)]
impl<T> Default for Fact<T> {
    fn default() -> Self {
        Self::Unknown
    }
}

impl<T: PartialEq> Fact<T> {
    /// Collapses candidates into a [`Fact`]. Candidates with an equal value are merged (keeping
    /// the most confident one); different values yield [`Fact::Conflicting`].
    #[must_use]
    pub fn from_candidates(candidates: Vec<Extracted<T>>) -> Self {
        let mut distinct: Vec<Extracted<T>> = Vec::new();
        for candidate in candidates {
            match distinct.iter_mut().find(|d| d.value == candidate.value) {
                Some(existing) => {
                    if candidate.confidence > existing.confidence {
                        *existing = candidate;
                    }
                }
                None => distinct.push(candidate),
            }
        }
        match distinct.len() {
            0 => Self::Unknown,
            1 => Self::Known(distinct.remove(0)),
            _ => Self::Conflicting(distinct),
        }
    }
}

impl<T> Fact<T> {
    /// The value only when it is unambiguous.
    #[must_use]
    pub const fn known(&self) -> Option<&Extracted<T>> {
        match self {
            Self::Known(e) => Some(e),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    #[must_use]
    pub const fn is_conflicting(&self) -> bool {
        matches!(self, Self::Conflicting(_))
    }
}

string_enum! {
    pub enum PropertyType {
        House => "house",
        /// *Casa geminada*.
        SemiDetached => "semi_detached",
        Sobrado => "sobrado",
        Apartment => "apartment",
        /// Bare plot (*lote* / *terreno*).
        Lot => "lot",
        /// *Chácara*: small rural property.
        Chacara => "chacara",
        Other => "other",
    }
}

string_enum! {
    /// How much of a plot is being sold, when the advertisement says so.
    pub enum LotShare {
        /// *Lote inteiro*.
        Whole => "whole",
        /// *Meio lote* / *1/2 lote*.
        Half => "half",
    }
}

/// Everything the app knows about a property's physical characteristics, from one text or
/// (later) from a consolidated property. Counts use `u8`; areas are the typed wrappers so lot,
/// built and usable area can never be swapped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyCharacteristics {
    pub property_type: Fact<PropertyType>,
    pub bedrooms: Fact<u8>,
    pub suites: Fact<u8>,
    pub parking_spaces: Fact<u8>,
    pub lot_area: Fact<LotArea>,
    pub built_area: Fact<BuiltArea>,
    pub usable_area: Fact<UsableArea>,
    /// Areas stated without saying which kind. Kept as a list and never promoted to another kind.
    pub unspecified_areas: Vec<Extracted<UnspecifiedArea>>,
    pub lot_share: Fact<LotShare>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::evidence::TextSpan;

    fn extracted(value: u8, confidence: Confidence) -> Extracted<u8> {
        Extracted {
            value,
            confidence,
            evidence: Evidence {
                span: TextSpan { start: 0, end: 1 },
                snippet: "x".into(),
            },
            warnings: vec![],
        }
    }

    #[test]
    fn no_candidates_is_unknown_not_zero() {
        assert!(Fact::<u8>::from_candidates(vec![]).is_unknown());
        assert!(PropertyCharacteristics::default().bedrooms.is_unknown());
    }

    #[test]
    fn equal_candidates_merge_keeping_the_most_confident() {
        let fact = Fact::from_candidates(vec![
            extracted(3, Confidence::Medium),
            extracted(3, Confidence::High),
        ]);
        let known = fact.known().expect("unambiguous");
        assert_eq!(known.value, 3);
        assert_eq!(known.confidence, Confidence::High);
    }

    #[test]
    fn different_candidates_are_a_conflict_with_all_evidence_kept() {
        let fact = Fact::from_candidates(vec![
            extracted(3, Confidence::High),
            extracted(2, Confidence::High),
        ]);
        assert!(fact.is_conflicting());
        assert!(fact.known().is_none());
        match fact {
            Fact::Conflicting(all) => assert_eq!(all.len(), 2),
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn serde_round_trip() {
        let fact = Fact::from_candidates(vec![extracted(3, Confidence::High)]);
        let json = serde_json::to_string(&fact).unwrap();
        assert_eq!(serde_json::from_str::<Fact<u8>>(&json).unwrap(), fact);
        let unknown: Fact<u8> = Fact::Unknown;
        let json = serde_json::to_string(&unknown).unwrap();
        assert_eq!(json, r#"{"status":"unknown"}"#);
        assert_eq!(serde_json::from_str::<Fact<u8>>(&json).unwrap(), unknown);
    }
}
