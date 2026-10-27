//! Types of the matching stage: how sure the matcher is, what was decided, and the evidence.
//!
//! Evidence keeps **every** signal considered, including those that disagree and those that
//! could not be assessed, so a decision can be reviewed later without re-running anything.

use serde::{Deserialize, Serialize};

string_enum! {
    /// The matcher's own categorisation of a pair. Matches `property_matches.confidence_category`.
    /// These are categories produced by an uncalibrated rule, not probabilities.
    pub enum MatchCategory {
        /// Several independent signals agree and none contradicts. Still reviewed by the user
        /// unless an evaluated automatic-merge rule exists (none does yet).
        High => "high",
        /// Worth a human look.
        Possible => "possible",
        /// Keep the two separate.
        Low => "low",
    }
}

string_enum! {
    /// Matches `property_matches.decision_status`.
    pub enum DecisionStatus {
        /// Considered and kept separate; nothing to review.
        Candidate => "candidate",
        Accepted => "accepted",
        Rejected => "rejected",
        /// Waiting for the user.
        ReviewRequired => "review_required",
    }
}

string_enum! {
    /// Who made a decision. Matches `property_matches.decision_source`.
    pub enum DecisionSource {
        Auto => "auto",
        User => "user",
    }
}

string_enum! {
    /// A kind of agreement between two listings.
    pub enum SignalKind {
        NearDuplicateImages => "near_duplicate_images",
        SameMunicipality => "same_municipality",
        SameNeighborhood => "same_neighborhood",
        SamePropertyType => "same_property_type",
        LotAreaMatch => "lot_area_match",
        BuiltAreaMatch => "built_area_match",
        UsableAreaMatch => "usable_area_match",
        RoomsMatch => "rooms_match",
        PriceClose => "price_close",
        TextSimilar => "text_similar",
    }
}

string_enum! {
    /// A kind of disagreement between two listings.
    pub enum ConflictKind {
        Municipality => "municipality",
        Neighborhood => "neighborhood",
        PropertyType => "property_type",
        LotArea => "lot_area",
        BuiltArea => "built_area",
        UsableArea => "usable_area",
        Bedrooms => "bedrooms",
        Suites => "suites",
        ParkingSpaces => "parking_spaces",
        Price => "price",
    }
}

string_enum! {
    pub enum Severity {
        /// A physical or locational contradiction: the two are probably different properties
        /// (or one ad is wrong). Never `High`; `Possible` only with strong image evidence.
        Hard => "hard",
        /// A disagreement that has innocent explanations (a repriced ad, an adjacent
        /// neighbourhood label). Never `High`.
        Soft => "soft",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    pub kind: SignalKind,
    /// Human-readable, with the values compared.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub kind: ConflictKind,
    pub severity: Severity,
    pub detail: String,
}

/// One image of listing A paired with one image of listing B whose hashes are close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePair {
    pub image_a: i64,
    pub image_b: i64,
    pub distance: u32,
}

/// Everything the matcher looked at for one pair. Stored as `property_matches.evidence_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchEvidence {
    pub rule_version: String,
    pub signals: Vec<Signal>,
    pub conflicts: Vec<Conflict>,
    /// What could not be compared because one or both sides do not state it. Absence of evidence
    /// is not evidence of a difference, and is not evidence of identity either.
    pub unknown: Vec<String>,
    pub image_pairs: Vec<ImagePair>,
    /// Anything else worth showing a reviewer.
    pub notes: Vec<String>,
}

impl MatchEvidence {
    #[must_use]
    pub fn has_signal(&self, kind: SignalKind) -> bool {
        self.signals.iter().any(|s| s.kind == kind)
    }

    #[must_use]
    pub fn has_conflict(&self, kind: ConflictKind) -> bool {
        self.conflicts.iter().any(|c| c.kind == kind)
    }

    #[must_use]
    pub fn has_hard_conflict(&self) -> bool {
        self.conflicts.iter().any(|c| c.severity == Severity::Hard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_forms_round_trip_and_reject_unknown_values() {
        macro_rules! round_trip {
            ($($ty:ty),+) => {$(
                for v in <$ty>::ALL {
                    assert_eq!(v.as_str().parse::<$ty>().unwrap(), *v);
                }
                assert!("nonsense".parse::<$ty>().is_err());
            )+};
        }
        round_trip!(
            MatchCategory,
            DecisionStatus,
            DecisionSource,
            SignalKind,
            ConflictKind,
            Severity
        );
    }

    #[test]
    fn evidence_survives_a_json_round_trip() {
        let evidence = MatchEvidence {
            rule_version: "match_rules/1".into(),
            signals: vec![Signal {
                kind: SignalKind::SameMunicipality,
                detail: "goiania".into(),
            }],
            conflicts: vec![Conflict {
                kind: ConflictKind::Price,
                severity: Severity::Soft,
                detail: "R$ 300.000 vs R$ 450.000".into(),
            }],
            unknown: vec!["bedrooms unknown in B".into()],
            image_pairs: vec![ImagePair {
                image_a: 1,
                image_b: 2,
                distance: 3,
            }],
            notes: vec![],
        };
        let json = serde_json::to_string(&evidence).unwrap();
        assert_eq!(
            serde_json::from_str::<MatchEvidence>(&json).unwrap(),
            evidence
        );
        assert!(json.contains("\"same_municipality\"") && json.contains("\"soft\""));
        assert!(evidence.has_signal(SignalKind::SameMunicipality));
        assert!(!evidence.has_signal(SignalKind::TextSimilar));
        assert!(evidence.has_conflict(ConflictKind::Price));
        assert!(!evidence.has_hard_conflict());
    }
}
