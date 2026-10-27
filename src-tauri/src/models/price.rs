use serde::{Deserialize, Serialize};

use super::{
    confidence::Confidence,
    evidence::{Evidence, ParseWarning},
    money::Money,
};

string_enum! {
    /// What an amount found in an advertisement actually *is*.
    ///
    /// Anything other than [`PriceClassification::FullPrice`] must never be compared against,
    /// averaged with or ranked as a property's price.
    pub enum PriceClassification {
        /// Total asking price of the property.
        FullPrice => "full_price",
        /// *Ágio*: what the seller wants for their equity in a financed property. Not a price.
        Agio => "agio",
        /// *Entrada*: an initial payment toward the price.
        DownPayment => "down_payment",
        /// A recurring instalment (*parcela*, *prestação*).
        MonthlyPayment => "monthly_payment",
        /// *Saldo devedor*: what remains owed on the financing.
        OutstandingBalance => "outstanding_balance",
        /// No supporting evidence for any of the above.
        Unknown => "unknown",
    }
}

macro_rules! typed_money {
    ($( $(#[$meta:meta])* $name:ident ),+ $(,)?) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
            #[serde(transparent)]
            pub struct $name(Money);

            impl $name {
                #[must_use]
                pub const fn new(amount: Money) -> Self {
                    Self(amount)
                }

                #[must_use]
                pub const fn money(self) -> Money {
                    self.0
                }
            }
        )+
    };
}

typed_money! {
    /// Total asking price. The only price type that may be compared across listings.
    FullPrice,
    /// Equity transfer amount; excludes the financed balance and any arrears.
    Agio,
    /// Initial payment toward the price.
    DownPayment,
    /// Recurring instalment.
    MonthlyPayment,
    /// Remaining financed balance.
    OutstandingBalance,
    /// An amount with no usable label.
    UnclassifiedAmount,
}

/// A monetary amount carrying its meaning in its type. Because every variant wraps a distinct
/// newtype, code that needs a [`FullPrice`] cannot be handed an [`Agio`] by mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "classification", content = "amount", rename_all = "snake_case")]
pub enum ClassifiedPrice {
    FullPrice(FullPrice),
    Agio(Agio),
    DownPayment(DownPayment),
    MonthlyPayment(MonthlyPayment),
    OutstandingBalance(OutstandingBalance),
    Unknown(UnclassifiedAmount),
}

impl ClassifiedPrice {
    #[must_use]
    pub const fn new(classification: PriceClassification, amount: Money) -> Self {
        match classification {
            PriceClassification::FullPrice => Self::FullPrice(FullPrice::new(amount)),
            PriceClassification::Agio => Self::Agio(Agio::new(amount)),
            PriceClassification::DownPayment => Self::DownPayment(DownPayment::new(amount)),
            PriceClassification::MonthlyPayment => {
                Self::MonthlyPayment(MonthlyPayment::new(amount))
            }
            PriceClassification::OutstandingBalance => {
                Self::OutstandingBalance(OutstandingBalance::new(amount))
            }
            PriceClassification::Unknown => Self::Unknown(UnclassifiedAmount::new(amount)),
        }
    }

    #[must_use]
    pub const fn classification(&self) -> PriceClassification {
        match self {
            Self::FullPrice(_) => PriceClassification::FullPrice,
            Self::Agio(_) => PriceClassification::Agio,
            Self::DownPayment(_) => PriceClassification::DownPayment,
            Self::MonthlyPayment(_) => PriceClassification::MonthlyPayment,
            Self::OutstandingBalance(_) => PriceClassification::OutstandingBalance,
            Self::Unknown(_) => PriceClassification::Unknown,
        }
    }

    /// The raw amount. Reading it back loses the type-level protection, so prefer matching on
    /// the variant when the meaning matters.
    #[must_use]
    pub const fn money(&self) -> Money {
        match self {
            Self::FullPrice(a) => a.money(),
            Self::Agio(a) => a.money(),
            Self::DownPayment(a) => a.money(),
            Self::MonthlyPayment(a) => a.money(),
            Self::OutstandingBalance(a) => a.money(),
            Self::Unknown(a) => a.money(),
        }
    }
}

/// One monetary amount found in an advertisement, with its classification, how sure we are,
/// and the text that supports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceMention {
    pub price: ClassifiedPrice,
    /// Confidence in the *classification* (the amount itself is parsed exactly).
    pub confidence: Confidence,
    /// Covers the label cue (when there is one) and the amount, e.g. `ágio de R$ 80 mil`.
    pub evidence: Evidence,
    pub warnings: Vec<ParseWarning>,
}

/// Minimum classification confidence for a full price to be used in comparisons and by the
/// "Ocultar Ágio (Apenas Valor Total)" filter.
pub const MIN_COMPARABLE_CONFIDENCE: Confidence = Confidence::Medium;

/// Whether a single advertisement states a full price we can rely on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FullPriceStatus {
    /// Exactly one distinct full-price amount supported with at least
    /// [`MIN_COMPARABLE_CONFIDENCE`].
    Established {
        price: FullPrice,
        confidence: Confidence,
    },
    /// Eligible full-price amounts disagree. Nothing is chosen on the caller's behalf.
    Conflicting { candidates: Vec<FullPrice> },
    /// No full price with enough support. Includes advertisements whose only amounts are
    /// ágio, down payment, instalments, balances or unlabelled numbers.
    NotEstablished,
}

impl FullPriceStatus {
    /// Behaviour of the user-facing filter **"Ocultar Ágio (Apenas Valor Total)"**: when it is
    /// on, only advertisements for which this returns `true` stay visible.
    #[must_use]
    pub const fn passes_full_price_only_filter(&self) -> bool {
        matches!(self, Self::Established { .. })
    }

    #[must_use]
    pub const fn established_price(&self) -> Option<FullPrice> {
        match self {
            Self::Established { price, .. } => Some(*price),
            _ => None,
        }
    }
}

/// Decides whether `mentions` establish a comparable full price.
///
/// Only [`ClassifiedPrice::FullPrice`] mentions with at least [`MIN_COMPARABLE_CONFIDENCE`]
/// count. Identical amounts are one price (keeping the best confidence); different amounts are
/// reported as a conflict rather than resolved by picking the lowest, highest or most confident.
#[must_use]
pub fn assess_full_price(mentions: &[PriceMention]) -> FullPriceStatus {
    // (amount, best confidence seen for that amount), in order of first appearance.
    let mut distinct: Vec<(FullPrice, Confidence)> = Vec::new();
    for mention in mentions {
        let ClassifiedPrice::FullPrice(price) = mention.price else {
            continue;
        };
        if mention.confidence < MIN_COMPARABLE_CONFIDENCE {
            continue;
        }
        match distinct.iter_mut().find(|(p, _)| *p == price) {
            Some((_, best)) => *best = (*best).max(mention.confidence),
            None => distinct.push((price, mention.confidence)),
        }
    }
    match distinct.as_slice() {
        [] => FullPriceStatus::NotEstablished,
        [(price, confidence)] => FullPriceStatus::Established {
            price: *price,
            confidence: *confidence,
        },
        _ => {
            let mut candidates: Vec<FullPrice> = distinct.into_iter().map(|(p, _)| p).collect();
            candidates.sort_by_key(|p| p.money());
            FullPriceStatus::Conflicting { candidates }
        }
    }
}

/// Where an observed price came from, stored with every `listing_price_history` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceProvenance {
    /// Identifier of the extractor and version, e.g. `br_real_estate/1`.
    pub extractor: String,
    /// Source field or page area the text came from, when known.
    pub source_field: Option<String>,
    pub evidence: Option<Evidence>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::evidence::TextSpan;

    fn mention(
        classification: PriceClassification,
        reais: i64,
        confidence: Confidence,
    ) -> PriceMention {
        PriceMention {
            price: ClassifiedPrice::new(classification, Money::from_reais(reais).unwrap()),
            confidence,
            evidence: Evidence {
                span: TextSpan { start: 0, end: 1 },
                snippet: "x".into(),
                field: None,
            },
            warnings: vec![],
        }
    }

    #[test]
    fn classified_price_round_trips_every_classification() {
        let amount = Money::from_reais(80_000).unwrap();
        for c in PriceClassification::ALL {
            let p = ClassifiedPrice::new(*c, amount);
            assert_eq!(p.classification(), *c);
            assert_eq!(p.money(), amount);
        }
    }

    #[test]
    fn same_amount_with_different_meaning_is_not_equal() {
        let amount = Money::from_reais(80_000).unwrap();
        assert_ne!(
            ClassifiedPrice::new(PriceClassification::Agio, amount),
            ClassifiedPrice::new(PriceClassification::FullPrice, amount)
        );
    }

    #[test]
    fn serde_uses_adjacent_tag() {
        let p = ClassifiedPrice::new(
            PriceClassification::Agio,
            Money::from_reais(80_000).unwrap(),
        );
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, r#"{"classification":"agio","amount":8000000}"#);
        assert_eq!(serde_json::from_str::<ClassifiedPrice>(&json).unwrap(), p);
    }

    #[test]
    fn single_medium_confidence_full_price_is_established() {
        let status = assess_full_price(&[mention(
            PriceClassification::FullPrice,
            350_000,
            Confidence::Medium,
        )]);
        assert_eq!(
            status,
            FullPriceStatus::Established {
                price: FullPrice::new(Money::from_reais(350_000).unwrap()),
                confidence: Confidence::Medium
            }
        );
        assert!(status.passes_full_price_only_filter());
    }

    #[test]
    fn low_confidence_full_price_is_only_a_candidate() {
        let status = assess_full_price(&[mention(
            PriceClassification::FullPrice,
            350_000,
            Confidence::Low,
        )]);
        assert_eq!(status, FullPriceStatus::NotEstablished);
        assert!(!status.passes_full_price_only_filter());
    }

    #[test]
    fn agio_down_payment_instalments_balance_and_unknown_never_establish_a_full_price() {
        for c in [
            PriceClassification::Agio,
            PriceClassification::DownPayment,
            PriceClassification::MonthlyPayment,
            PriceClassification::OutstandingBalance,
            PriceClassification::Unknown,
        ] {
            let status = assess_full_price(&[mention(c, 350_000, Confidence::High)]);
            assert_eq!(
                status,
                FullPriceStatus::NotEstablished,
                "{c} must not become a full price"
            );
        }
    }

    #[test]
    fn full_price_alongside_agio_is_still_established_from_the_full_price_alone() {
        let status = assess_full_price(&[
            mention(PriceClassification::Agio, 80_000, Confidence::High),
            mention(PriceClassification::FullPrice, 350_000, Confidence::High),
        ]);
        assert_eq!(
            status.established_price().unwrap().money(),
            Money::from_reais(350_000).unwrap()
        );
    }

    #[test]
    fn repeated_identical_full_prices_are_one_price_with_the_best_confidence() {
        let status = assess_full_price(&[
            mention(PriceClassification::FullPrice, 350_000, Confidence::Medium),
            mention(PriceClassification::FullPrice, 350_000, Confidence::High),
        ]);
        assert_eq!(
            status,
            FullPriceStatus::Established {
                price: FullPrice::new(Money::from_reais(350_000).unwrap()),
                confidence: Confidence::High
            }
        );
    }

    #[test]
    fn disagreeing_full_prices_are_reported_not_resolved() {
        let status = assess_full_price(&[
            mention(PriceClassification::FullPrice, 360_000, Confidence::High),
            mention(PriceClassification::FullPrice, 350_000, Confidence::Medium),
        ]);
        assert_eq!(
            status,
            FullPriceStatus::Conflicting {
                candidates: vec![
                    FullPrice::new(Money::from_reais(350_000).unwrap()),
                    FullPrice::new(Money::from_reais(360_000).unwrap()),
                ]
            }
        );
        assert!(!status.passes_full_price_only_filter());
    }

    #[test]
    fn no_mentions_means_not_established() {
        assert_eq!(assess_full_price(&[]), FullPriceStatus::NotEstablished);
    }

    #[test]
    fn text_form_round_trips_and_rejects_unknown_values() {
        for c in PriceClassification::ALL {
            assert_eq!(c.as_str().parse::<PriceClassification>().unwrap(), *c);
        }
        assert!("discount".parse::<PriceClassification>().is_err());
    }
}
