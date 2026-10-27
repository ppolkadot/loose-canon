use serde::{Deserialize, Serialize};

/// Half-open byte range `[start, end)` into the **original, unmodified** advertisement text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
}

/// The piece of source text that justifies an extracted value, kept so the UI and later
/// reviews can show *why* the app believes something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub span: TextSpan,
    /// Exact slice of the original text covered by `span`.
    pub snippet: String,
    /// Source field the text came from (`description`, `price`...) when the caller named one.
    /// `span` indexes into **that field's** text. `None` for plain single-text parsing, and for
    /// data stored before fields existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl Evidence {
    /// Builds evidence from a span of `original`.
    ///
    /// # Panics
    /// Panics if the span is out of bounds or not on `char` boundaries. The parser only builds
    /// spans from its own offset map, which guarantees valid boundaries.
    #[must_use]
    pub fn from_span(original: &str, span: TextSpan) -> Self {
        Self {
            snippet: original[span.start..span.end].to_owned(),
            span,
            field: None,
        }
    }

    #[must_use]
    pub fn with_field(mut self, field: Option<String>) -> Self {
        self.field = field;
        self
    }
}

string_enum! {
    /// Machine-readable reason attached to an extracted value that deserves a caveat.
    pub enum WarningKind {
        /// An amount appeared with no label saying what it is.
        UnlabelledAmount => "unlabelled_amount",
        /// An unlabelled amount was provisionally treated as the full price because it is larger
        /// than every labelled financing component. Low confidence by construction.
        AssumedFullPrice => "assumed_full_price",
        /// Cues on both sides of an amount point to different meanings.
        ConflictingLabels => "conflicting_labels",
        /// The amount is labelled as something that is not a sale price (IPTU, condominium, rent...).
        NonSaleAmount => "non_sale_amount",
        /// A `.` was read as a decimal separator (`1.5 milhão`), which is not the Brazilian norm.
        DotAsDecimalSeparator => "dot_as_decimal_separator",
        /// `2 quartos + suíte` style wording: the bedroom count may exclude the suites.
        SuitesMayBeAdditionalToBedrooms => "suites_may_be_additional_to_bedrooms",
        /// A bare singular word (`suíte`) was read as a count of one.
        CountImpliedBySingular => "count_implied_by_singular",
        /// The area is described generically (`área`, `área total`) so its kind is unknown.
        UnspecifiedAreaKind => "unspecified_area_kind",
        /// The amount carries no label of its own; it was taken as the asking price only because
        /// it sits in the source's structured price field.
        PriceFromStructuredField => "price_from_structured_field",
    }
}

/// A caveat on one extracted value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseWarning {
    pub kind: WarningKind,
    /// Short user-facing explanation in Brazilian Portuguese.
    pub message: String,
}

impl ParseWarning {
    #[must_use]
    pub fn new(kind: WarningKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

string_enum! {
    /// Why a number or phrase that *looked* like a fact was deliberately not extracted.
    pub enum DiscardReason {
        /// `300 m da avenida`: a distance or other linear measure, not an area.
        LinearMeasureNotArea => "linear_measure_not_area",
        /// `3/4 de lote`: a fraction of a plot, not a bedroom count.
        FractionOfLot => "fraction_of_lot",
        /// `3/4 do valor`: a fraction of something, not a bedroom count.
        FractionNotBedrooms => "fraction_not_bedrooms",
        /// `3/4` with no surrounding wording that makes it a bedroom count.
        BedroomShorthandWithoutContext => "bedroom_shorthand_without_context",
        /// `3/4 quartos` could be the shorthand or a "3 to 4 bedrooms" range.
        AmbiguousBedroomRange => "ambiguous_bedroom_range",
        /// `300 a 400 mil`: the first number has no magnitude word, so it is not trustworthy.
        AmbiguousAmountRange => "ambiguous_amount_range",
        /// Hectares, alqueires... are not converted because the conversion is not universal.
        UnsupportedAreaUnit => "unsupported_area_unit",
        /// `suítes` with no number.
        PluralWithoutCount => "plural_without_count",
        /// A count outside the believable range for a house or apartment.
        ImplausibleCount => "implausible_count",
        /// The digits could not be turned into an exact value.
        UnparseableNumber => "unparseable_number",
    }
}

/// Something the parser saw and consciously ignored, kept for transparency and debugging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardedMention {
    pub reason: DiscardReason,
    pub evidence: Evidence,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_slices_the_original_text() {
        let text = "ágio de R$ 80 mil";
        // 'á' is two bytes, so byte offsets differ from char offsets.
        let span = TextSpan {
            start: 0,
            end: "ágio".len(),
        };
        let ev = Evidence::from_span(text, span);
        assert_eq!(ev.snippet, "ágio");
        assert_eq!(ev.span, span);
        assert_eq!(ev.field, None);
    }

    #[test]
    fn field_is_optional_in_json_so_older_stored_evidence_still_loads() {
        let old: Evidence =
            serde_json::from_str(r#"{"span":{"start":0,"end":4},"snippet":"ágio"}"#).unwrap();
        assert_eq!(old.field, None);

        let ev = Evidence::from_span(
            "ágio",
            TextSpan {
                start: 0,
                end: "ágio".len(),
            },
        )
        .with_field(Some("price".into()));
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""field":"price""#));
        assert_eq!(serde_json::from_str::<Evidence>(&json).unwrap(), ev);

        let plain =
            serde_json::to_string(&Evidence::from_span("ágio", TextSpan { start: 0, end: 5 }))
                .unwrap();
        assert!(
            !plain.contains("field"),
            "an absent field must not be serialised: {plain}"
        );
    }
}
