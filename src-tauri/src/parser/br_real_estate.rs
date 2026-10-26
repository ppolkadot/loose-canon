//! Parser for Brazilian real-estate advertisement text.
//!
//! The parser is deterministic and conservative: it would rather leave a fact `Unknown`, flag
//! it, or move it to `discarded` than guess. Every extracted value keeps the original text that
//! supports it ([`Evidence`]) and a [`Confidence`].
//!
//! # How classification works
//!
//! Text is folded to lowercase ASCII (offsets stay mapped to the original) and scanned for
//! *candidates* (amounts, counts, areas). Each candidate is then classified from its
//! **context** only:
//!
//! * a *left context* window (bounded by the previous candidate and by sentence boundaries),
//! * a *right context* of attached wording (`80 mil de ágio`, `2.500 por mês`).
//!
//! The label **nearest** to the candidate wins; contradicting labels on both sides produce
//! `Unknown` + a warning. Regexes only locate cues and numbers; what a number *means* is decided
//! by the surrounding label, never by the digits.
//!
//! # Known limits
//!
//! * Unlabelled amounts stay `Unknown`. The only exception is an unlabelled amount larger than
//!   every labelled financing component (`R$ 350 mil ou entrada de R$ 80 mil`), which becomes a
//!   *low confidence* full-price candidate and so never passes the full-price-only filter.
//! * Bare digits (no `R$`, no `mil`/`k`) are an amount only next to a label, with a thousands
//!   separator or at least four digits (`parcelas de 500` is therefore not read).
//! * Hectares and alqueires are reported as discarded, not converted.
//! * Text is assumed to be Brazilian Portuguese; other languages are not interpreted.

use std::{ops::Range, sync::LazyLock};

use regex::Regex;

use super::{
    number::{self, Magnitude},
    text::FoldedText,
};
use crate::models::{
    area::{Area, AreaKind, BuiltArea, LotArea, UnspecifiedArea, UsableArea},
    characteristics::{Extracted, Fact, LotShare, PropertyCharacteristics},
    confidence::Confidence,
    evidence::{DiscardReason, DiscardedMention, ParseWarning, WarningKind},
    listing::ExtractedFacts,
    price::{ClassifiedPrice, PriceClassification, PriceMention},
};

/// Identifies this extractor and its behaviour version in stored provenance.
pub const PARSER_VERSION: &str = "br_real_estate/1";

/// How far left of a candidate a label may sit.
const PRICE_LEFT_WINDOW: usize = 48;
const AREA_LEFT_WINDOW: usize = 30;
/// How far right of a candidate attached wording is looked for.
const RIGHT_WINDOW: usize = 40;
/// A label closer than this (in bytes of folded text) is considered directly attached.
const ADJACENT_GAP: usize = 14;
/// Unlabelled amounts below R$ 10.000 are never promoted to full-price candidates.
const MIN_PROMOTABLE_FULL_PRICE_CENTAVOS: i64 = 1_000_000;
/// Bedroom, suite and parking counts above this are treated as parsing noise.
const MAX_PLAUSIBLE_COUNT: u32 = 20;

/// Compiles a regex once per call site.
macro_rules! regex {
    ($pattern:expr) => {{
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("valid static regex"));
        &*RE
    }};
}

/// Parses one advertisement text (callers may join title and description with a newline).
#[must_use]
pub fn parse_listing_text(text: &str) -> ExtractedFacts {
    let folded = FoldedText::new(text);
    let mut discarded = Vec::new();
    let mut prices = extract_prices(&folded, &mut discarded);
    promote_unlabelled_amounts(&mut prices);
    let characteristics = extract_characteristics(&folded, &mut discarded);
    ExtractedFacts {
        parser_version: PARSER_VERSION.to_owned(),
        prices,
        characteristics,
        discarded,
    }
}

// ================================================================================================
// Shared context helpers
// ================================================================================================

/// Sentence-level boundaries that stop a label from leaking onto a neighbouring amount.
fn sentence_boundary() -> &'static Regex {
    regex!(r"[\n;|!?•]|\.\s|\.$")
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i.min(s.len())
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Where the left-context window for a candidate starting at `start` begins.
fn left_context_start(s: &str, start: usize, floor: usize, window: usize) -> usize {
    let mut from = ceil_char_boundary(s, start.saturating_sub(window).max(floor));
    if let Some(boundary) = sentence_boundary().find_iter(&s[from..start]).last() {
        from += boundary.end();
    }
    from
}

/// Text right after a candidate that may carry attached wording, cut at sentence boundaries.
fn right_context(s: &str, end: usize) -> &str {
    let to = floor_char_boundary(s, end + RIGHT_WINDOW);
    let window = &s[end..to];
    sentence_boundary()
        .find(window)
        .map_or(window, |boundary| &window[..boundary.start()])
}

/// A cue found in a context window, with offsets into the whole folded text.
#[derive(Debug, Clone)]
struct Hit<L> {
    label: L,
    range: Range<usize>,
}

struct LeftCue<L> {
    label: L,
    pattern: Regex,
    /// When the text right after the cue matches this, the cue is a different phrase
    /// (`entrada independente` is a door, not a down payment).
    reject_if_followed_by: Option<Regex>,
}

fn left_cue<L>(label: L, pattern: &str) -> LeftCue<L> {
    LeftCue {
        label,
        pattern: Regex::new(pattern).expect("valid static regex"),
        reject_if_followed_by: None,
    }
}

/// The cue closest to the candidate (ties: higher `priority`, then longer).
fn nearest_hit<L: Copy>(
    cues: &[LeftCue<L>],
    context: &str,
    context_start: usize,
    priority: impl Fn(L) -> u8,
) -> Option<Hit<L>> {
    let mut best: Option<(usize, u8, usize, Hit<L>)> = None;
    for cue in cues {
        for found in cue.pattern.find_iter(context) {
            if let Some(reject) = &cue.reject_if_followed_by
                && reject.is_match(&context[found.end()..])
            {
                continue;
            }
            let key = (found.end(), priority(cue.label), found.len());
            if best.as_ref().is_none_or(|(e, p, l, _)| key > (*e, *p, *l)) {
                best = Some((
                    key.0,
                    key.1,
                    key.2,
                    Hit {
                        label: cue.label,
                        range: context_start + found.start()..context_start + found.end(),
                    },
                ));
            }
        }
    }
    best.map(|(.., hit)| hit)
}

/// First attached-wording pattern matching at the very start of `context`.
fn attached_hit<L: Copy>(
    cues: &[(L, Regex)],
    context: &str,
    context_start: usize,
) -> Option<Hit<L>> {
    cues.iter().find_map(|(label, pattern)| {
        pattern.find(context).map(|found| Hit {
            label: *label,
            range: context_start + found.start()..context_start + found.end(),
        })
    })
}

const fn union(a: &Range<usize>, b: &Range<usize>) -> Range<usize> {
    let start = if a.start < b.start { a.start } else { b.start };
    let end = if a.end > b.end { a.end } else { b.end };
    start..end
}

fn discard(
    folded: &FoldedText<'_>,
    discarded: &mut Vec<DiscardedMention>,
    reason: DiscardReason,
    range: Range<usize>,
) {
    discarded.push(DiscardedMention {
        reason,
        evidence: folded.evidence(range),
    });
}

// ================================================================================================
// Prices
// ================================================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PriceLabel {
    Agio,
    DownPayment,
    MonthlyPayment,
    OutstandingBalance,
    /// Bare `saldo`: almost always the remaining balance, but weaker than `saldo devedor`.
    BalanceGeneric,
    /// `valor total`, `preço total`: explicitly the whole price.
    FullStrong,
    /// `preço`, `valor`, `venda`: probably the price, but also appears in `valor da parcela`.
    FullGeneric,
    /// Condominium, IPTU, rent, financing...: an amount, but not a sale price.
    NonSale,
}

impl PriceLabel {
    const fn classification(self) -> PriceClassification {
        match self {
            Self::Agio => PriceClassification::Agio,
            Self::DownPayment => PriceClassification::DownPayment,
            Self::MonthlyPayment => PriceClassification::MonthlyPayment,
            Self::OutstandingBalance | Self::BalanceGeneric => {
                PriceClassification::OutstandingBalance
            }
            Self::FullStrong | Self::FullGeneric => PriceClassification::FullPrice,
            Self::NonSale => PriceClassification::Unknown,
        }
    }

    const fn is_full(self) -> bool {
        matches!(self, Self::FullStrong | Self::FullGeneric)
    }

    /// Tie-break when two cues end at the same place: the more specific one wins.
    const fn priority(self) -> u8 {
        match self {
            Self::FullGeneric | Self::BalanceGeneric => 1,
            Self::FullStrong => 2,
            _ => 3,
        }
    }

    /// Cues that are suggestive rather than definitive never reach `High` confidence.
    const fn is_generic(self) -> bool {
        matches!(self, Self::FullGeneric | Self::BalanceGeneric)
    }
}

fn price_left_cues() -> &'static [LeftCue<PriceLabel>] {
    static CUES: LazyLock<Vec<LeftCue<PriceLabel>>> = LazyLock::new(|| {
        let mut entrada = left_cue(PriceLabel::DownPayment, r"\bentrada\b");
        entrada.reject_if_followed_by = Some(
            Regex::new(r"^\s+(?:independente|social|de\s+servico|principal|lateral|de\s+veiculos|coberta)\b")
                .expect("valid static regex"),
        );
        vec![
            left_cue(PriceLabel::Agio, r"\bagio\b"),
            entrada,
            left_cue(PriceLabel::MonthlyPayment, r"\bparcelas?\b"),
            left_cue(PriceLabel::MonthlyPayment, r"\bprestac(?:ao|oes)\b"),
            left_cue(PriceLabel::MonthlyPayment, r"\bmensalidades?\b"),
            left_cue(PriceLabel::OutstandingBalance, r"\bsaldo\s+devedor\b"),
            left_cue(PriceLabel::BalanceGeneric, r"\bsaldo\b"),
            left_cue(
                PriceLabel::FullStrong,
                r"\b(?:valor|preco)\s+(?:total|integral)\b",
            ),
            left_cue(PriceLabel::FullGeneric, r"\b(?:preco|valor|venda)\b"),
            left_cue(PriceLabel::FullGeneric, r"\bvend(?:o|e)\b"),
            left_cue(PriceLabel::FullGeneric, r"\ba\s+vista\b"),
            left_cue(
                PriceLabel::NonSale,
                r"\b(?:condominio|iptu|itbi|taxas?|alugue(?:l|is)|locacao|financiamento|financiad[oa]|comissao|fgts)\b",
            ),
        ]
    });
    &CUES
}

/// Wording attached *after* an amount. A preposition is required for noun cues so that
/// `R$ 350 mil entrada de R$ 80 mil` does not hand the second amount's label to the first.
fn price_right_cues() -> &'static [(PriceLabel, Regex)] {
    static CUES: LazyLock<Vec<(PriceLabel, Regex)>> = LazyLock::new(|| {
        let cue = |label, pattern: &str| (label, Regex::new(pattern).expect("valid static regex"));
        vec![
            cue(
                PriceLabel::Agio,
                r"^\s*(?:de|em|no|na|do|da|a\s+titulo\s+de)\s+agio\b",
            ),
            cue(
                PriceLabel::DownPayment,
                r"^\s*(?:de|em|no|na|a\s+titulo\s+de)\s+entrada\b",
            ),
            cue(
                PriceLabel::OutstandingBalance,
                r"^\s*(?:de|em|no|na)\s+saldo\s+devedor\b",
            ),
            cue(
                PriceLabel::MonthlyPayment,
                r"^\s*(?:(?:/|por|ao|a|cada)\s*mes\b|(?:mensais|mensal)\b|(?:de|em)\s+(?:parcelas?|prestac(?:ao|oes)|mensalidades?)\b)",
            ),
            cue(
                PriceLabel::NonSale,
                r"^\s*(?:de|do|da)\s+(?:condominio|iptu|taxa)\b",
            ),
            cue(PriceLabel::FullGeneric, r"^\s*(?:a\s+vista|total)\b"),
        ]
    });
    &CUES
}

struct AmountMatch<'t> {
    range: Range<usize>,
    token: &'t str,
    has_currency: bool,
    magnitude: Option<Magnitude>,
    has_reais_word: bool,
}

fn scan_amounts(s: &str) -> Vec<AmountMatch<'_>> {
    regex!(
        r"(?P<cur>\br\$\s*)?(?P<num>[0-9]{1,3}(?:\.[0-9]{3})+(?:,[0-9]+)?|[0-9]+(?:[.,][0-9]+)?)(?:\s*(?P<mag>milhoes|milhao|mil|k)\b)?(?:\s*(?P<reais>reais|real)\b)?"
    )
    .captures_iter(s)
    .filter_map(|caps| {
        let whole = caps.get(0)?;
        let token = caps.name("num")?.as_str();
        Some(AmountMatch {
            range: whole.range(),
            token,
            has_currency: caps.name("cur").is_some(),
            magnitude: caps
                .name("mag")
                .and_then(|m| Magnitude::from_folded_word(m.as_str())),
            has_reais_word: caps.name("reais").is_some(),
        })
    })
    .collect()
}

/// Whether the text between a label and a bare number is just glue (`de`, `:`, `-`...).
fn is_adjacent_glue(gap: &str) -> bool {
    regex!(r"^\s*(?:(?:de|do|da|em|a)\s+)?[:=\-]?\s*$").is_match(gap)
}

/// Bare digits followed by a unit are not money (`1.250 m2`, `300 metros`, `2 quartos`).
fn followed_by_unit(rest: &str) -> bool {
    regex!(
        r"^\s*(?:m2|mt2|mts2|mq|m\b|mts?\b|metros?\b|km\b|quilometros?\b|quartos?\b|vagas?\b|suites?\b|%|hectares?\b|ha\b|alqueires?\b)"
    )
    .is_match(rest)
}

/// `300 a 400 mil`: the first number has lost its magnitude word.
fn starts_amount_range(rest: &str) -> bool {
    regex!(r"^\s*(?:a|ate|ou|-)\s*(?:r\$\s*)?[0-9][0-9.,]*\s*(?:milhoes|milhao|mil|k)\b")
        .is_match(rest)
}

enum Resolution {
    Unlabelled,
    Conflict {
        evidence: Range<usize>,
    },
    Labelled {
        label: PriceLabel,
        confidence: Confidence,
        evidence: Range<usize>,
    },
}

fn left_confidence(label: PriceLabel, gap: usize) -> Confidence {
    match (label.is_generic(), gap <= ADJACENT_GAP) {
        (true, true) | (false, false) => Confidence::Medium,
        (true, false) => Confidence::Low,
        (false, true) => Confidence::High,
    }
}

fn resolve_price_label(
    amount: &Range<usize>,
    left: Option<&Hit<PriceLabel>>,
    right: Option<&Hit<PriceLabel>>,
) -> Resolution {
    let gap = |hit: &Hit<PriceLabel>| amount.start.saturating_sub(hit.range.end);
    match (left, right) {
        (None, None) => Resolution::Unlabelled,
        (Some(l), None) => Resolution::Labelled {
            label: l.label,
            confidence: left_confidence(l.label, gap(l)),
            evidence: union(&l.range, amount),
        },
        (None, Some(r)) => Resolution::Labelled {
            label: r.label,
            confidence: if r.label.is_full() {
                Confidence::Medium
            } else {
                Confidence::High
            },
            evidence: union(&r.range, amount),
        },
        (Some(l), Some(r)) => {
            let both = union(&union(&l.range, &r.range), amount);
            if l.label == r.label || (l.label.is_full() && r.label.is_full()) {
                let label = if l.label == PriceLabel::FullStrong {
                    l.label
                } else {
                    r.label
                };
                Resolution::Labelled {
                    label,
                    confidence: Confidence::High,
                    evidence: both,
                }
            } else if l.label == PriceLabel::FullGeneric && !r.label.is_full() {
                // `valor R$ 80 mil de ágio`: the attached wording is the specific one.
                Resolution::Labelled {
                    label: r.label,
                    confidence: Confidence::High,
                    evidence: both,
                }
            } else if r.label.is_full() && !l.label.is_full() {
                // `entrada de R$ 80 mil à vista`: keep the specific left label.
                Resolution::Labelled {
                    label: l.label,
                    confidence: left_confidence(l.label, gap(l)),
                    evidence: both,
                }
            } else {
                Resolution::Conflict { evidence: both }
            }
        }
    }
}

fn warning_for(kind: WarningKind) -> ParseWarning {
    let message = match kind {
        WarningKind::UnlabelledAmount => {
            "Valor sem indicação do que representa (preço, ágio, entrada ou parcela)."
        }
        WarningKind::AssumedFullPrice => {
            "Valor sem rótulo e maior que os demais valores informados; tratado apenas como possível preço total."
        }
        WarningKind::ConflictingLabels => {
            "Termos contraditórios ao redor do valor; não foi possível classificá-lo."
        }
        WarningKind::NonSaleAmount => {
            "Valor associado a outra cobrança (condomínio, IPTU, aluguel, financiamento...); não é preço de venda."
        }
        WarningKind::DotAsDecimalSeparator => {
            "Ponto interpretado como separador decimal; confirme o valor."
        }
        WarningKind::SuitesMayBeAdditionalToBedrooms => {
            "O texto sugere suíte(s) além dos quartos; o total de quartos pode ser maior."
        }
        WarningKind::CountImpliedBySingular => {
            "Quantidade deduzida do singular (\"suíte\"); o anúncio não informa o número."
        }
        WarningKind::UnspecifiedAreaKind => {
            "O anúncio não informa se a área é de lote, construída ou útil."
        }
    };
    ParseWarning::new(kind, message)
}

fn extract_prices(
    folded: &FoldedText<'_>,
    discarded: &mut Vec<DiscardedMention>,
) -> Vec<PriceMention> {
    let s = folded.as_str();
    let mut mentions = Vec::new();
    // Nothing before this offset may serve as left context (previous amount / consumed wording).
    let mut floor = 0;

    for amount in scan_amounts(s) {
        if amount.range.start < floor {
            continue;
        }
        let context_start = left_context_start(s, amount.range.start, floor, PRICE_LEFT_WINDOW);
        let left = nearest_hit(
            price_left_cues(),
            &s[context_start..amount.range.start],
            context_start,
            PriceLabel::priority,
        );
        let right_ctx = right_context(s, amount.range.end);
        let right = attached_hit(price_right_cues(), right_ctx, amount.range.end);

        let is_money_shaped =
            amount.has_currency || amount.magnitude.is_some() || amount.has_reais_word;
        if !is_money_shaped {
            // Bare digits: only next to a label, only with a thousands separator or >= 4 digits.
            let adjacent_label = left.as_ref().is_some_and(|hit| {
                hit.label != PriceLabel::NonSale
                    && is_adjacent_glue(&s[hit.range.end..amount.range.start])
            });
            let looks_like_money = amount.token.contains('.') || amount.token.len() >= 4;
            if !(adjacent_label && looks_like_money) || followed_by_unit(&s[amount.range.end..]) {
                continue;
            }
        }

        if amount.magnitude.is_none() && starts_amount_range(&s[amount.range.end..]) {
            discard(
                folded,
                discarded,
                DiscardReason::AmbiguousAmountRange,
                amount.range.clone(),
            );
            floor = amount.range.end;
            continue;
        }

        let Ok(parsed) = number::amount_from_parts(amount.token, amount.magnitude) else {
            discard(
                folded,
                discarded,
                DiscardReason::UnparseableNumber,
                amount.range.clone(),
            );
            floor = amount.range.end;
            continue;
        };

        let (classification, confidence, mut warnings, evidence_range) =
            match resolve_price_label(&amount.range, left.as_ref(), right.as_ref()) {
                Resolution::Unlabelled => (
                    PriceClassification::Unknown,
                    Confidence::Low,
                    vec![warning_for(WarningKind::UnlabelledAmount)],
                    amount.range.clone(),
                ),
                Resolution::Conflict { evidence } => (
                    PriceClassification::Unknown,
                    Confidence::Low,
                    vec![warning_for(WarningKind::ConflictingLabels)],
                    evidence,
                ),
                Resolution::Labelled {
                    label,
                    confidence,
                    evidence,
                } => {
                    let warnings = if label == PriceLabel::NonSale {
                        vec![warning_for(WarningKind::NonSaleAmount)]
                    } else {
                        Vec::new()
                    };
                    (label.classification(), confidence, warnings, evidence)
                }
            };
        if parsed.dot_as_decimal {
            warnings.push(warning_for(WarningKind::DotAsDecimalSeparator));
        }

        floor = right
            .as_ref()
            .map_or(amount.range.end, |hit| hit.range.end.max(amount.range.end));
        mentions.push(PriceMention {
            price: ClassifiedPrice::new(classification, parsed.money),
            confidence,
            evidence: folded.evidence(evidence_range),
            warnings,
        });
    }
    mentions
}

/// An unlabelled amount that is larger than every labelled financing component of the same
/// advertisement is most plausibly the asking price. It is recorded as a **low confidence**
/// full-price candidate (never comparable, never passes the full-price-only filter) so the
/// information is not lost, while the warning says it is an assumption.
fn promote_unlabelled_amounts(mentions: &mut [PriceMention]) {
    let largest_component = mentions
        .iter()
        .filter(|m| {
            matches!(
                m.price.classification(),
                PriceClassification::Agio
                    | PriceClassification::DownPayment
                    | PriceClassification::MonthlyPayment
                    | PriceClassification::OutstandingBalance
            ) && m.confidence >= Confidence::Medium
        })
        .map(|m| m.price.money())
        .max();
    let Some(largest_component) = largest_component else {
        return;
    };
    for mention in mentions {
        let unlabelled = mention.price.classification() == PriceClassification::Unknown
            && mention
                .warnings
                .iter()
                .any(|w| w.kind == WarningKind::UnlabelledAmount);
        let amount = mention.price.money();
        if unlabelled
            && amount > largest_component
            && amount.centavos() >= MIN_PROMOTABLE_FULL_PRICE_CENTAVOS
        {
            mention.price = ClassifiedPrice::new(PriceClassification::FullPrice, amount);
            mention.confidence = Confidence::Low;
            mention.warnings = vec![warning_for(WarningKind::AssumedFullPrice)];
        }
    }
}

// ================================================================================================
// Characteristics
// ================================================================================================

fn extract_characteristics(
    folded: &FoldedText<'_>,
    discarded: &mut Vec<DiscardedMention>,
) -> PropertyCharacteristics {
    let mut consumed: Vec<Range<usize>> = Vec::new();
    let lot_share = extract_lot_share(folded, discarded, &mut consumed);
    let bedrooms = extract_bedrooms(folded, discarded, &consumed);
    let suites = extract_suites(folded, discarded, !bedrooms.is_unknown());
    let parking_spaces = extract_parking(folded, discarded);
    let areas = extract_areas(folded, discarded);
    PropertyCharacteristics {
        property_type: Fact::Unknown,
        bedrooms,
        suites,
        parking_spaces,
        lot_area: Fact::from_candidates(areas.lot),
        built_area: Fact::from_candidates(areas.built),
        usable_area: Fact::from_candidates(areas.usable),
        unspecified_areas: areas.unspecified,
        lot_share,
    }
}

fn extracted<T>(
    folded: &FoldedText<'_>,
    value: T,
    confidence: Confidence,
    range: Range<usize>,
    warnings: Vec<ParseWarning>,
) -> Extracted<T> {
    Extracted {
        value,
        confidence,
        evidence: folded.evidence(range),
        warnings,
    }
}

fn spelled_or_digit_count(token: &str) -> Option<u32> {
    match token.trim() {
        "um" | "uma" => Some(1),
        "dois" | "duas" => Some(2),
        "tres" => Some(3),
        "quatro" => Some(4),
        "cinco" => Some(5),
        "seis" => Some(6),
        "sete" => Some(7),
        "oito" => Some(8),
        digits => digits.parse().ok(),
    }
}

/// A plausible count, or a `discarded` record explaining why not.
fn count_candidate(
    folded: &FoldedText<'_>,
    discarded: &mut Vec<DiscardedMention>,
    token: &str,
    confidence: Confidence,
    range: Range<usize>,
    warnings: Vec<ParseWarning>,
) -> Option<Extracted<u8>> {
    let plausible = spelled_or_digit_count(token)
        .filter(|n| (1..=MAX_PLAUSIBLE_COUNT).contains(n))
        .and_then(|n| u8::try_from(n).ok());
    if let Some(value) = plausible {
        Some(extracted(folded, value, confidence, range, warnings))
    } else {
        discard(folded, discarded, DiscardReason::ImplausibleCount, range);
        None
    }
}

fn overlaps(range: &Range<usize>, others: &[Range<usize>]) -> bool {
    others
        .iter()
        .any(|o| range.start < o.end && o.start < range.end)
}

/// The previous non-space character is `/` (so we are looking at the tail of a fraction).
fn preceded_by_slash(s: &str, start: usize) -> bool {
    s[..start].trim_end().ends_with('/')
}

fn extract_lot_share(
    folded: &FoldedText<'_>,
    discarded: &mut Vec<DiscardedMention>,
    consumed: &mut Vec<Range<usize>>,
) -> Fact<LotShare> {
    let s = folded.as_str();
    let mut candidates = Vec::new();
    let push = |range: Range<usize>, share: LotShare, candidates: &mut Vec<Extracted<LotShare>>| {
        candidates.push(extracted(
            folded,
            share,
            Confidence::High,
            range,
            Vec::new(),
        ));
    };

    for found in regex!(r"\bmeio\s+lote\b|\bmetade\s+(?:do\s+|de\s+um\s+)?lote\b").find_iter(s) {
        push(found.range(), LotShare::Half, &mut candidates);
    }
    for found in regex!(r"\blote\s+inteiro\b").find_iter(s) {
        push(found.range(), LotShare::Whole, &mut candidates);
    }
    for caps in regex!(
        r"\b(?P<n>[0-9]{1,2})\s*/\s*(?P<d>[0-9]{1,2})\s*(?:(?:de|do|da)\s+)?(?:(?:um|o)\s+)?(?:lote|terreno)\b"
    )
    .captures_iter(s)
    {
        let Some(whole) = caps.get(0) else { continue };
        consumed.push(whole.range());
        if &caps["n"] == "1" && &caps["d"] == "2" {
            push(whole.range(), LotShare::Half, &mut candidates);
        } else {
            discard(folded, discarded, DiscardReason::FractionOfLot, whole.range());
        }
    }
    Fact::from_candidates(candidates)
}

fn extract_bedrooms(
    folded: &FoldedText<'_>,
    discarded: &mut Vec<DiscardedMention>,
    consumed: &[Range<usize>],
) -> Fact<u8> {
    let s = folded.as_str();
    let mut candidates = Vec::new();

    for caps in regex!(
        r"\b(?P<n>[0-9]{1,3}|um|uma|dois|duas|tres|quatro|cinco|seis|sete|oito)\s*(?:quartos?|dormitorios?|dorms?|qtos?|qts?)\b"
    )
    .captures_iter(s)
    {
        let Some(whole) = caps.get(0) else { continue };
        // `3/4 quartos`: the "4 quartos" tail belongs to an ambiguous fraction, handled below.
        if preceded_by_slash(s, whole.start()) || overlaps(&whole.range(), consumed) {
            continue;
        }
        candidates.extend(count_candidate(
            folded,
            discarded,
            &caps["n"],
            Confidence::High,
            whole.range(),
            Vec::new(),
        ));
    }

    for caps in regex!(r"\b(?P<n>[0-9]{1,2})\s*/\s*(?P<d>[0-9]{1,2})\b").captures_iter(s) {
        let Some(whole) = caps.get(0) else { continue };
        let range = whole.range();
        let after = &s[range.end..];
        // Dates (`03/04/2024`) and fractions already explained as lot shares are not bedrooms.
        if overlaps(&range, consumed) || after.starts_with('/') || preceded_by_slash(s, range.start)
        {
            continue;
        }
        if regex!(r"^\s*(?:quartos?|dormitorios?)\b").is_match(after) {
            discard(
                folded,
                discarded,
                DiscardReason::AmbiguousBedroomRange,
                range,
            );
            continue;
        }
        if &caps["d"] != "4" {
            continue;
        }
        if regex!(r"^\s*(?:de|do|da|dos|das)\b").is_match(after) {
            discard(folded, discarded, DiscardReason::FractionNotBedrooms, range);
            continue;
        }
        let from = ceil_char_boundary(s, range.start.saturating_sub(40));
        let to = floor_char_boundary(s, range.end + 40);
        let window = &s[from..to];
        if !regex!(
            r"\b(?:quartos?|suites?|vagas?|garagem|banheiros?|wc|sala|cozinha|casa|sobrado|apartamento|apto|kitnet|residencia|sendo)\b"
        )
        .is_match(window)
        {
            discard(folded, discarded, DiscardReason::BedroomShorthandWithoutContext, range);
            continue;
        }
        let strong_context =
            regex!(r"\b(?:quartos?|suites?|vagas?|garagem|banheiros?|wc|sala|cozinha)\b")
                .is_match(window);
        let confidence = if strong_context {
            Confidence::High
        } else {
            Confidence::Medium
        };
        candidates.extend(count_candidate(
            folded,
            discarded,
            &caps["n"],
            confidence,
            range,
            Vec::new(),
        ));
    }
    Fact::from_candidates(candidates)
}

fn extract_suites(
    folded: &FoldedText<'_>,
    discarded: &mut Vec<DiscardedMention>,
    bedrooms_present: bool,
) -> Fact<u8> {
    let s = folded.as_str();
    let mut candidates = Vec::new();
    let mut numeric_ranges: Vec<Range<usize>> = Vec::new();
    let additive_warning = |start: usize| -> Vec<ParseWarning> {
        if bedrooms_present && s[..start].trim_end().ends_with('+') {
            vec![warning_for(WarningKind::SuitesMayBeAdditionalToBedrooms)]
        } else {
            Vec::new()
        }
    };

    for caps in regex!(r"\b(?P<n>[0-9]{1,3}\s*|(?:um|uma|dois|duas|tres|quatro|cinco)\s+)suites?\b")
        .captures_iter(s)
    {
        let Some(whole) = caps.get(0) else { continue };
        numeric_ranges.push(whole.range());
        candidates.extend(count_candidate(
            folded,
            discarded,
            &caps["n"],
            Confidence::High,
            whole.range(),
            additive_warning(whole.start()),
        ));
    }

    let has_numeric_candidate = !candidates.is_empty();
    for found in regex!(r"\bsuites?\b").find_iter(s) {
        if overlaps(&found.range(), &numeric_ranges) {
            continue;
        }
        if found.as_str() == "suite" {
            if !has_numeric_candidate {
                let mut warnings = vec![warning_for(WarningKind::CountImpliedBySingular)];
                warnings.extend(additive_warning(found.start()));
                candidates.push(extracted(
                    folded,
                    1,
                    Confidence::Medium,
                    found.range(),
                    warnings,
                ));
            }
        } else {
            discard(
                folded,
                discarded,
                DiscardReason::PluralWithoutCount,
                found.range(),
            );
        }
    }
    Fact::from_candidates(candidates)
}

fn extract_parking(folded: &FoldedText<'_>, discarded: &mut Vec<DiscardedMention>) -> Fact<u8> {
    let s = folded.as_str();
    let mut candidates = Vec::new();
    for caps in regex!(
        r"\b(?P<n>[0-9]{1,3}\s*|(?:um|uma|dois|duas|tres|quatro|cinco)\s+)(?:vagas?|garagem|garagens)\b"
    )
    .captures_iter(s)
    {
        let Some(whole) = caps.get(0) else { continue };
        candidates.extend(count_candidate(
            folded,
            discarded,
            &caps["n"],
            Confidence::High,
            whole.range(),
            Vec::new(),
        ));
    }
    for caps in regex!(
        r"\b(?:vagas?|garagem|garagens)\s+(?:cobertas?\s+)?(?:para|pra|p/)\s*(?P<n>[0-9]{1,3}|um|uma|dois|duas|tres|quatro|cinco)\s*(?:carros?|veiculos?|autos?)\b"
    )
    .captures_iter(s)
    {
        let Some(whole) = caps.get(0) else { continue };
        candidates.extend(count_candidate(
            folded,
            discarded,
            &caps["n"],
            Confidence::High,
            whole.range(),
            Vec::new(),
        ));
    }
    Fact::from_candidates(candidates)
}

// ------------------------------------------------------------------------------------------------
// Areas
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AreaLabel {
    Built,
    Lot,
    Usable,
    /// `área`, `área total`: says "area" but not which kind.
    Generic,
}

impl AreaLabel {
    const fn kind(self) -> AreaKind {
        match self {
            Self::Built => AreaKind::Built,
            Self::Lot => AreaKind::Lot,
            Self::Usable => AreaKind::Usable,
            Self::Generic => AreaKind::Unspecified,
        }
    }

    const fn priority(self) -> u8 {
        match self {
            Self::Generic => 1,
            _ => 2,
        }
    }
}

fn area_left_cues() -> &'static [LeftCue<AreaLabel>] {
    static CUES: LazyLock<Vec<LeftCue<AreaLabel>>> = LazyLock::new(|| {
        vec![
            left_cue(
                AreaLabel::Built,
                r"\b(?:area\s+)?(?:construida|edificada)\b|\bconstrucao\b|\bconstruidos?\b",
            ),
            left_cue(AreaLabel::Lot, r"\b(?:lote|terreno)\b"),
            left_cue(
                AreaLabel::Usable,
                r"\barea\s+(?:util|interna|privativa)\b|\bprivativa\b",
            ),
            left_cue(AreaLabel::Generic, r"\barea(?:\s+total)?\b"),
        ]
    });
    &CUES
}

fn area_right_cues() -> &'static [(AreaLabel, Regex)] {
    static CUES: LazyLock<Vec<(AreaLabel, Regex)>> = LazyLock::new(|| {
        let cue = |label, pattern: &str| (label, Regex::new(pattern).expect("valid static regex"));
        vec![
            // Adjectives attach directly (`180 m2 construidos`); nouns need a preposition.
            cue(
                AreaLabel::Built,
                r"^\s*(?:construid[oa]s?|edificad[oa]s?)\b",
            ),
            cue(
                AreaLabel::Built,
                r"^\s*(?:de|do|da)\s+(?:area\s+)?(?:construid[oa]s?|construcao|edificad[oa]s?)\b",
            ),
            cue(AreaLabel::Lot, r"^\s*(?:de|do|da)\s+(?:lote|terreno)\b"),
            cue(
                AreaLabel::Usable,
                r"^\s*(?:uteis|privativ[oa]s?|intern[oa]s?)\b",
            ),
            cue(
                AreaLabel::Usable,
                r"^\s*(?:de|do|da)\s+area\s+(?:util|interna|privativa)\b",
            ),
            cue(AreaLabel::Generic, r"^\s*(?:de|do|da)\s+area\s+total\b"),
        ]
    });
    &CUES
}

#[derive(Default)]
struct AreaCandidates {
    lot: Vec<Extracted<LotArea>>,
    built: Vec<Extracted<BuiltArea>>,
    usable: Vec<Extracted<UsableArea>>,
    unspecified: Vec<Extracted<UnspecifiedArea>>,
}

/// A valid area plus whether its decimal mark was a `.`.
fn parse_area(token: &str) -> Option<(Area, bool)> {
    let (centi, dot_as_decimal) = number::centi_square_meters_from_token(token).ok()?;
    Some((Area::from_centi_square_meters(centi).ok()?, dot_as_decimal))
}

/// Records numbers that look like areas but are not: distances (`300m da avenida`) and units
/// we deliberately do not convert (hectares, alqueires).
fn report_non_area_measures(
    folded: &FoldedText<'_>,
    discarded: &mut Vec<DiscardedMention>,
    area_ranges: &[Range<usize>],
) {
    let s = folded.as_str();
    for found in regex!(
        r"(?:[0-9]{1,3}(?:\.[0-9]{3})+|[0-9]+(?:,[0-9]+)?)\s*(?:km|quilometros?|mts?|metros?|m)\b"
    )
    .find_iter(s)
    {
        // `300 metros quadrados` is an area and is handled by the area scan.
        if overlaps(&found.range(), area_ranges)
            || regex!(r"^\s*quadrad[oa]s?\b").is_match(&s[found.end()..])
        {
            continue;
        }
        discard(
            folded,
            discarded,
            DiscardReason::LinearMeasureNotArea,
            found.range(),
        );
    }
    for found in regex!(
        r"(?:[0-9]+(?:[.,][0-9]+)?|um|uma|meio|meia|dois|duas|tres|quatro|cinco)\s*(?:hectares?|ha|alqueires?)\b"
    )
    .find_iter(s)
    {
        discard(folded, discarded, DiscardReason::UnsupportedAreaUnit, found.range());
    }
}

fn extract_areas(folded: &FoldedText<'_>, discarded: &mut Vec<DiscardedMention>) -> AreaCandidates {
    let s = folded.as_str();
    let area_matches: Vec<(Range<usize>, &str)> = regex!(
        r"(?P<num>[0-9]{1,3}(?:\.[0-9]{3})+(?:,[0-9]+)?|[0-9]+(?:[.,][0-9]+)?)\s*(?:m2|mt2|mts2|mq|metros?\s+quadrados?)\b"
    )
    .captures_iter(s)
    .filter_map(|caps| Some((caps.get(0)?.range(), caps.name("num")?.as_str())))
    .collect();
    let area_ranges: Vec<Range<usize>> = area_matches.iter().map(|(r, _)| r.clone()).collect();
    report_non_area_measures(folded, discarded, &area_ranges);

    let mut out = AreaCandidates::default();
    let mut floor = 0;
    for (range, token) in area_matches {
        let Some((area, dot_as_decimal)) = parse_area(token) else {
            discard(
                folded,
                discarded,
                DiscardReason::UnparseableNumber,
                range.clone(),
            );
            floor = range.end;
            continue;
        };

        let context_start = left_context_start(s, range.start, floor, AREA_LEFT_WINDOW);
        let left = nearest_hit(
            area_left_cues(),
            &s[context_start..range.start],
            context_start,
            AreaLabel::priority,
        );
        // Wording right after the area, up to the next digit (which starts another candidate).
        let right_ctx = right_context(s, range.end);
        let right_ctx = right_ctx
            .find(|c: char| c.is_ascii_digit())
            .map_or(right_ctx, |digit| &right_ctx[..digit]);
        let right = attached_hit(area_right_cues(), right_ctx, range.end);

        let (kind, confidence, mut warnings, evidence_range) =
            resolve_area_label(&range, left.as_ref(), right.as_ref());
        if kind == AreaKind::Unspecified {
            warnings.push(warning_for(WarningKind::UnspecifiedAreaKind));
        }
        if dot_as_decimal {
            warnings.push(warning_for(WarningKind::DotAsDecimalSeparator));
        }
        floor = right
            .as_ref()
            .map_or(range.end, |hit| hit.range.end.max(range.end));

        match kind {
            AreaKind::Lot => out.lot.push(extracted(
                folded,
                LotArea::new(area),
                confidence,
                evidence_range,
                warnings,
            )),
            AreaKind::Built => {
                out.built.push(extracted(
                    folded,
                    BuiltArea::new(area),
                    confidence,
                    evidence_range,
                    warnings,
                ));
            }
            AreaKind::Usable => {
                out.usable.push(extracted(
                    folded,
                    UsableArea::new(area),
                    confidence,
                    evidence_range,
                    warnings,
                ));
            }
            AreaKind::Unspecified => out.unspecified.push(extracted(
                folded,
                UnspecifiedArea::new(area),
                confidence,
                evidence_range,
                warnings,
            )),
        }
    }
    out
}

fn resolve_area_label(
    area: &Range<usize>,
    left: Option<&Hit<AreaLabel>>,
    right: Option<&Hit<AreaLabel>>,
) -> (AreaKind, Confidence, Vec<ParseWarning>, Range<usize>) {
    let gap = |hit: &Hit<AreaLabel>| area.start.saturating_sub(hit.range.end);
    let left_confidence = |hit: &Hit<AreaLabel>| {
        if hit.label == AreaLabel::Generic {
            Confidence::Low
        } else if gap(hit) <= ADJACENT_GAP {
            Confidence::High
        } else {
            Confidence::Medium
        }
    };
    let unspecified = |evidence| (AreaKind::Unspecified, Confidence::Low, Vec::new(), evidence);
    match (left, right) {
        (None, None) => unspecified(area.clone()),
        (Some(l), None) => (
            l.label.kind(),
            left_confidence(l),
            Vec::new(),
            union(&l.range, area),
        ),
        (None, Some(r)) => {
            let confidence = if r.label == AreaLabel::Generic {
                Confidence::Low
            } else {
                Confidence::High
            };
            (
                r.label.kind(),
                confidence,
                Vec::new(),
                union(&r.range, area),
            )
        }
        (Some(l), Some(r)) => {
            let both = union(&union(&l.range, &r.range), area);
            if l.label == r.label {
                let confidence = if l.label == AreaLabel::Generic {
                    Confidence::Low
                } else {
                    Confidence::High
                };
                (l.label.kind(), confidence, Vec::new(), both)
            } else if l.label == AreaLabel::Generic {
                (r.label.kind(), Confidence::High, Vec::new(), both)
            } else if r.label == AreaLabel::Generic {
                (l.label.kind(), left_confidence(l), Vec::new(), both)
            } else {
                // `lote 300 m2 de construção`: both sides name a different kind.
                (
                    AreaKind::Unspecified,
                    Confidence::Low,
                    vec![warning_for(WarningKind::ConflictingLabels)],
                    both,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests;
