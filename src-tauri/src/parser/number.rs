//! Brazilian number and money normalisation (`.` groups thousands, `,` is the decimal mark).
//!
//! Pure functions over already-folded text. Nothing here decides whether a number *is* a price;
//! that needs context and lives in `br_real_estate`.

use std::sync::LazyLock;

use regex::Regex;

use super::text::FoldedText;
use crate::{error::ParseError, models::money::Money};

/// Largest amount accepted as plausible (R$ 1 bilhão). Catches mis-parsed digit strings.
pub const MAX_PLAUSIBLE_REAIS: i64 = 1_000_000_000;

/// Magnitude words that multiply a number: `350 mil`, `1,2 milhão`, `350k`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Magnitude {
    Thousand,
    Million,
}

impl Magnitude {
    /// Recognises the **folded** (lowercase, accent-free) word.
    #[must_use]
    pub fn from_folded_word(word: &str) -> Option<Self> {
        match word {
            "mil" | "k" => Some(Self::Thousand),
            "milhao" | "milhoes" => Some(Self::Million),
            _ => None,
        }
    }
}

/// Digits of a number split at its decimal mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberParts {
    pub int_digits: String,
    pub frac_digits: String,
    /// The decimal mark was a `.` (e.g. `1.5`), which is not the Brazilian convention.
    pub dot_as_decimal: bool,
}

/// An exact amount plus a note about how ambiguous its notation was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedAmount {
    pub money: Money,
    pub dot_as_decimal: bool,
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `1.234`, `12.345.678`: a 1-3 digit head followed by one or more `.ddd` groups.
fn is_grouped_thousands(s: &str) -> bool {
    let mut groups = s.split('.');
    let Some(head) = groups.next() else {
        return false;
    };
    if !(1..=3).contains(&head.len()) || !all_digits(head) {
        return false;
    }
    let mut tail_groups = 0;
    for group in groups {
        if group.len() != 3 || !all_digits(group) {
            return false;
        }
        tail_groups += 1;
    }
    tail_groups >= 1
}

/// Splits a token made of digits, `.` and `,` according to the Brazilian convention.
///
/// * `1.234.567,89` -> int `1234567`, frac `89`
/// * `1.234` -> thousands group (never `1.234` as a decimal)
/// * `1.5`, `350.00` -> `.` as a decimal mark, flagged because it is not the Brazilian norm
/// * `1,234.56` (US style), `1.2.3`, `1,2,3` -> error: ambiguous, so not guessed
pub fn split_number(token: &str) -> Result<NumberParts, ParseError> {
    let invalid = || ParseError::InvalidNumber(token.to_owned());
    if token.is_empty()
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b',')
    {
        return Err(invalid());
    }
    let dots = token.matches('.').count();
    let commas = token.matches(',').count();
    let parts = |int: &str, frac: &str, dot_as_decimal: bool| NumberParts {
        int_digits: int.to_owned(),
        frac_digits: frac.to_owned(),
        dot_as_decimal,
    };
    match (dots, commas) {
        (0, 0) => Ok(parts(token, "", false)),
        (_, 0) => {
            if is_grouped_thousands(token) {
                Ok(parts(&token.replace('.', ""), "", false))
            } else if dots == 1 {
                let (int, frac) = token.split_once('.').ok_or_else(invalid)?;
                if all_digits(int) && all_digits(frac) && frac.len() <= 2 {
                    Ok(parts(int, frac, true))
                } else {
                    Err(invalid())
                }
            } else {
                Err(invalid())
            }
        }
        (0, 1) => {
            let (int, frac) = token.split_once(',').ok_or_else(invalid)?;
            if all_digits(int) && all_digits(frac) {
                Ok(parts(int, frac, false))
            } else {
                Err(invalid())
            }
        }
        (_, 1) => {
            let (int, frac) = token.split_once(',').ok_or_else(invalid)?;
            if is_grouped_thousands(int) && all_digits(frac) {
                Ok(parts(&int.replace('.', ""), frac, false))
            } else {
                Err(invalid())
            }
        }
        _ => Err(invalid()),
    }
}

/// Exact value of `token` (optionally scaled by a magnitude word) in centavos.
///
/// Uses integer arithmetic only. Anything that cannot be represented exactly in centavos
/// (`R$ 350.000,555`) is rejected rather than rounded.
pub fn amount_from_parts(
    token: &str,
    magnitude: Option<Magnitude>,
) -> Result<ParsedAmount, ParseError> {
    let parts = split_number(token)?;
    if parts.int_digits.len() > 15 {
        return Err(ParseError::OutOfRange);
    }
    if parts.frac_digits.len() > 12 {
        return Err(ParseError::TooPrecise(token.to_owned()));
    }
    let int: i128 = parts
        .int_digits
        .parse()
        .map_err(|_| ParseError::OutOfRange)?;
    let frac: i128 = if parts.frac_digits.is_empty() {
        0
    } else {
        parts
            .frac_digits
            .parse()
            .map_err(|_| ParseError::OutOfRange)?
    };
    let scale =
        10_i128.pow(u32::try_from(parts.frac_digits.len()).map_err(|_| ParseError::OutOfRange)?);
    let centavos_per_unit: i128 = match magnitude {
        None => 100,
        Some(Magnitude::Thousand) => 100_000,
        Some(Magnitude::Million) => 100_000_000,
    };
    let numerator = (int * scale + frac) * centavos_per_unit;
    if numerator % scale != 0 {
        return Err(ParseError::TooPrecise(token.to_owned()));
    }
    let centavos = numerator / scale;
    if centavos == 0 {
        return Err(ParseError::ZeroAmount);
    }
    if centavos > i128::from(MAX_PLAUSIBLE_REAIS) * 100 {
        return Err(ParseError::OutOfRange);
    }
    let centavos = i64::try_from(centavos).map_err(|_| ParseError::OutOfRange)?;
    let money = Money::from_centavos(centavos).map_err(|_| ParseError::OutOfRange)?;
    Ok(ParsedAmount {
        money,
        dot_as_decimal: parts.dot_as_decimal,
    })
}

/// Converts an area token (`180`, `180,5`, `1.250`) to hundredths of a square metre.
/// The flag reports a `.` decimal mark.
pub fn centi_square_meters_from_token(token: &str) -> Result<(u64, bool), ParseError> {
    let parts = split_number(token)?;
    if parts.int_digits.len() > 12 {
        return Err(ParseError::OutOfRange);
    }
    if parts.frac_digits.len() > 2 {
        return Err(ParseError::TooPrecise(token.to_owned()));
    }
    let int: u64 = parts
        .int_digits
        .parse()
        .map_err(|_| ParseError::OutOfRange)?;
    let frac: u64 = match parts.frac_digits.len() {
        0 => 0,
        1 => {
            parts
                .frac_digits
                .parse::<u64>()
                .map_err(|_| ParseError::OutOfRange)?
                * 10
        }
        _ => parts
            .frac_digits
            .parse()
            .map_err(|_| ParseError::OutOfRange)?,
    };
    Ok((int * 100 + frac, parts.dot_as_decimal))
}

/// Parses a standalone Brazilian money string such as `R$ 350.000,00`, `350 mil`,
/// `350 mil reais`, `350k`, `R$ 1,2 milhão` or `1.200.000`.
///
/// This answers "what amount does this string denote?", not "is it a price?": callers that
/// scan free text must still establish context (see `br_real_estate`).
pub fn parse_money(input: &str) -> Result<Money, ParseError> {
    static MONEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^\s*(?:r\$\s*)?(?P<num>[0-9.,]+)(?:\s*(?P<mag>milhoes|milhao|mil|k))?(?:\s*(?:reais|real))?\s*$",
        )
        .expect("valid static regex")
    });

    if input.trim().is_empty() {
        return Err(ParseError::Empty);
    }
    let folded = FoldedText::new(input);
    let caps = MONEY
        .captures(folded.as_str())
        .ok_or_else(|| ParseError::UnrecognizedFormat(input.to_owned()))?;
    let magnitude = caps
        .name("mag")
        .and_then(|m| Magnitude::from_folded_word(m.as_str()));
    let token = caps.name("num").map_or("", |m| m.as_str());
    Ok(amount_from_parts(token, magnitude)?.money)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centavos(input: &str) -> i64 {
        parse_money(input)
            .unwrap_or_else(|e| panic!("{input:?}: {e}"))
            .centavos()
    }

    #[test]
    fn parses_every_supported_notation_exactly() {
        assert_eq!(centavos("R$ 350.000,00"), 35_000_000);
        assert_eq!(centavos("R$350.000"), 35_000_000);
        assert_eq!(centavos("r$ 350.000,00"), 35_000_000);
        assert_eq!(centavos("350 mil"), 35_000_000);
        assert_eq!(centavos("350 mil reais"), 35_000_000);
        assert_eq!(centavos("350k"), 35_000_000);
        assert_eq!(centavos("350K"), 35_000_000);
        assert_eq!(centavos("R$ 1,2 milhão"), 120_000_000);
        assert_eq!(centavos("R$ 1,2 milhões"), 120_000_000);
        assert_eq!(centavos("1.200.000"), 120_000_000);
        assert_eq!(centavos("R$ 2.500"), 250_000);
        assert_eq!(centavos("R$ 2.500,50"), 250_050);
        assert_eq!(centavos("350000"), 35_000_000);
        assert_eq!(centavos("R$ 80 mil"), 8_000_000);
        assert_eq!(centavos("R$ 349,90"), 34_990);
        assert_eq!(centavos("2 milhões"), 200_000_000);
    }

    #[test]
    fn dot_as_decimal_is_accepted_only_where_unambiguous_and_flagged() {
        let amount = amount_from_parts("1.5", Some(Magnitude::Million)).unwrap();
        assert_eq!(amount.money.centavos(), 150_000_000);
        assert!(amount.dot_as_decimal);
        // Exactly three digits after a dot is a thousands group, never a decimal.
        let amount = amount_from_parts("1.500", None).unwrap();
        assert_eq!(amount.money.centavos(), 150_000);
        assert!(!amount.dot_as_decimal);
    }

    #[test]
    fn rejects_input_it_cannot_represent_exactly() {
        assert_eq!(parse_money(""), Err(ParseError::Empty));
        assert_eq!(parse_money("   "), Err(ParseError::Empty));
        assert!(matches!(
            parse_money("abc"),
            Err(ParseError::UnrecognizedFormat(_))
        ));
        assert!(matches!(
            parse_money("-5"),
            Err(ParseError::UnrecognizedFormat(_))
        ));
        assert_eq!(parse_money("R$ 0"), Err(ParseError::ZeroAmount));
        assert!(matches!(
            parse_money("R$ 350.000,555"),
            Err(ParseError::TooPrecise(_))
        ));
        assert!(matches!(
            parse_money("R$ 1,234.56"),
            Err(ParseError::InvalidNumber(_))
        ));
        assert!(matches!(
            parse_money("R$ 1.2.3"),
            Err(ParseError::InvalidNumber(_))
        ));
        assert!(matches!(
            parse_money("R$ 12.34.567"),
            Err(ParseError::InvalidNumber(_))
        ));
        assert_eq!(
            parse_money("R$ 99999999999999"),
            Err(ParseError::OutOfRange)
        );
        assert_eq!(parse_money("2000 milhões"), Err(ParseError::OutOfRange));
    }

    #[test]
    fn splits_numbers_by_brazilian_convention() {
        let p = split_number("350.000,00").unwrap();
        assert_eq!(
            (
                p.int_digits.as_str(),
                p.frac_digits.as_str(),
                p.dot_as_decimal
            ),
            ("350000", "00", false)
        );
        let p = split_number("1.200.000").unwrap();
        assert_eq!(
            (p.int_digits.as_str(), p.frac_digits.as_str()),
            ("1200000", "")
        );
        let p = split_number("180,5").unwrap();
        assert_eq!(
            (p.int_digits.as_str(), p.frac_digits.as_str()),
            ("180", "5")
        );
        let p = split_number("180.5").unwrap();
        assert_eq!(
            (
                p.int_digits.as_str(),
                p.frac_digits.as_str(),
                p.dot_as_decimal
            ),
            ("180", "5", true)
        );
        assert!(split_number("").is_err());
        assert!(split_number("1,2,3").is_err());
    }

    #[test]
    fn converts_area_tokens_to_hundredths() {
        assert_eq!(
            centi_square_meters_from_token("180").unwrap(),
            (18_000, false)
        );
        assert_eq!(
            centi_square_meters_from_token("180,5").unwrap(),
            (18_050, false)
        );
        assert_eq!(
            centi_square_meters_from_token("180,55").unwrap(),
            (18_055, false)
        );
        assert_eq!(
            centi_square_meters_from_token("1.250").unwrap(),
            (125_000, false)
        );
        assert_eq!(
            centi_square_meters_from_token("180.5").unwrap(),
            (18_050, true)
        );
        assert!(centi_square_meters_from_token("180,555").is_err());
    }
}
