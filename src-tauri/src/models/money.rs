use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::ModelError;

string_enum! {
    /// Currency of a stored amount. Only BRL is supported; the column exists so that the
    /// representation is explicit rather than implied.
    pub enum Currency {
        Brl => "BRL",
    }
}

/// An exact, non-negative amount of Brazilian reais stored as integer centavos.
///
/// No floating point is involved anywhere, so equality and ordering are exact. `Money` has no
/// arithmetic operators on purpose: the typed wrappers in [`super::price`] decide which amounts
/// may be combined or compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct Money(i64);

impl Money {
    pub fn from_centavos(centavos: i64) -> Result<Self, ModelError> {
        if centavos < 0 {
            return Err(ModelError::NegativeMoney(centavos));
        }
        Ok(Self(centavos))
    }

    pub fn from_reais(reais: i64) -> Result<Self, ModelError> {
        let centavos = reais.checked_mul(100).ok_or(ModelError::MoneyOverflow)?;
        Self::from_centavos(centavos)
    }

    #[must_use]
    pub const fn centavos(self) -> i64 {
        self.0
    }

    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }
}

impl TryFrom<i64> for Money {
    type Error = ModelError;

    fn try_from(centavos: i64) -> Result<Self, Self::Error> {
        Self::from_centavos(centavos)
    }
}

impl From<Money> for i64 {
    fn from(money: Money) -> Self {
        money.0
    }
}

/// Formats as the Brazilian convention, e.g. `R$ 350.000,00`.
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reais = (self.0 / 100).to_string();
        let mut grouped = String::with_capacity(reais.len() + reais.len() / 3);
        for (i, digit) in reais.chars().enumerate() {
            if i > 0 && (reais.len() - i).is_multiple_of(3) {
                grouped.push('.');
            }
            grouped.push(digit);
        }
        write!(f, "R$ {grouped},{:02}", self.0 % 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_exact_centavos() {
        assert_eq!(Money::from_reais(350_000).unwrap().centavos(), 35_000_000);
        assert_eq!(Money::from_centavos(1).unwrap().centavos(), 1);
    }

    #[test]
    fn rejects_negative_and_overflowing_amounts() {
        assert_eq!(Money::from_centavos(-1), Err(ModelError::NegativeMoney(-1)));
        assert_eq!(Money::from_reais(i64::MAX), Err(ModelError::MoneyOverflow));
    }

    #[test]
    fn displays_in_brazilian_format() {
        assert_eq!(
            Money::from_reais(350_000).unwrap().to_string(),
            "R$ 350.000,00"
        );
        assert_eq!(
            Money::from_centavos(250_050).unwrap().to_string(),
            "R$ 2.500,50"
        );
        assert_eq!(Money::from_centavos(5).unwrap().to_string(), "R$ 0,05");
        assert_eq!(
            Money::from_reais(1_200_000).unwrap().to_string(),
            "R$ 1.200.000,00"
        );
    }

    #[test]
    fn serde_round_trip_validates_on_the_way_in() {
        let m = Money::from_reais(80_000).unwrap();
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, "8000000");
        assert_eq!(serde_json::from_str::<Money>(&json).unwrap(), m);
        assert!(serde_json::from_str::<Money>("-5").is_err());
    }
}
