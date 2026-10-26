//! Case/accent folding with an offset map back to the original text.
//!
//! Advertisements are matched against a lowercase, accent-free copy so the regexes stay
//! simple (`agio`, `suite`, `m2`). Evidence, however, must quote the **original** text, so every
//! byte of the folded copy remembers which original bytes it came from.

use std::ops::Range;

use crate::models::evidence::{Evidence, TextSpan};

pub struct FoldedText<'a> {
    original: &'a str,
    folded: String,
    /// For every byte of `folded`: `[start, end)` of the original char it came from.
    spans: Vec<(usize, usize)>,
}

const fn is_combining_mark(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}')
}

/// One-to-one character folding (after lowercasing). Keeping it one-to-one is what makes the
/// byte offset map cheap and exact.
fn fold_char(c: char) -> char {
    match c {
        '\n' => '\n',
        c if c.is_whitespace() => ' ',
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        '²' => '2',
        '³' => '3',
        '×' => 'x',
        '–' | '—' | '−' => '-',
        '’' | '‘' => '\'',
        other => other,
    }
}

impl<'a> FoldedText<'a> {
    #[must_use]
    pub fn new(original: &'a str) -> Self {
        let mut folded = String::with_capacity(original.len());
        let mut spans: Vec<(usize, usize)> = Vec::with_capacity(original.len());
        // Index into `spans` where the most recently emitted character starts.
        let mut last_char_first_byte: Option<usize> = None;

        for (start, c) in original.char_indices() {
            let end = start + c.len_utf8();
            if is_combining_mark(c) {
                // Dropped from the folded text, but the previous character's evidence must
                // still cover it so a quote never cuts a letter from its accent.
                if let Some(first) = last_char_first_byte {
                    for span in &mut spans[first..] {
                        span.1 = end;
                    }
                }
                continue;
            }
            last_char_first_byte = Some(spans.len());
            for lower in c.to_lowercase() {
                let folded_char = fold_char(lower);
                folded.push(folded_char);
                spans.extend(std::iter::repeat_n((start, end), folded_char.len_utf8()));
            }
        }
        Self {
            original,
            folded,
            spans,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.folded
    }

    /// Original-text span covering `range` of the folded text.
    #[must_use]
    pub fn span(&self, range: Range<usize>) -> TextSpan {
        if range.is_empty() {
            let at = self
                .spans
                .get(range.start)
                .map_or(self.original.len(), |s| s.0);
            return TextSpan { start: at, end: at };
        }
        TextSpan {
            start: self.spans[range.start].0,
            end: self.spans[range.end - 1].1,
        }
    }

    #[must_use]
    pub fn evidence(&self, range: Range<usize>) -> Evidence {
        Evidence::from_span(self.original, self.span(range))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_accents_and_superscripts() {
        let f = FoldedText::new("ÁGIO de R$ 80 Mil — área construída 180 m² Suíte");
        assert_eq!(
            f.as_str(),
            "agio de r$ 80 mil - area construida 180 m2 suite"
        );
    }

    #[test]
    fn folds_every_portuguese_diacritic() {
        let f = FoldedText::new("çãõáàâäéèêëíìîïóòôöúùûü");
        assert_eq!(f.as_str(), "caoaaaaeeeeiiiioooouuuu");
    }

    #[test]
    fn treats_non_breaking_space_as_space_and_keeps_newlines() {
        let f = FoldedText::new("R$\u{a0}80\tmil\nentrada");
        assert_eq!(f.as_str(), "r$ 80 mil\nentrada");
    }

    #[test]
    fn maps_folded_ranges_back_to_original_bytes() {
        let text = "ágio de R$ 80 mil";
        let f = FoldedText::new(text);
        let start = f.as_str().find("agio").unwrap();
        let ev = f.evidence(start..start + 4);
        assert_eq!(ev.snippet, "ágio");
        assert_eq!(
            ev.span,
            TextSpan {
                start: 0,
                end: "ágio".len()
            }
        );

        let start = f.as_str().find("80").unwrap();
        let ev = f.evidence(start..start + 2);
        assert_eq!(ev.snippet, "80");
        assert_eq!(&text[ev.span.start..ev.span.end], "80");
    }

    #[test]
    fn evidence_stays_on_char_boundaries_with_multibyte_and_emoji_text() {
        let text = "🏠 Casa — ÁREA CONSTRUÍDA de 180 m²!";
        let f = FoldedText::new(text);
        let s = f.as_str().find("area construida").unwrap();
        let ev = f.evidence(s..s + "area construida".len());
        assert_eq!(ev.snippet, "ÁREA CONSTRUÍDA");
    }

    #[test]
    fn decomposed_accents_do_not_break_the_offset_map() {
        // "ágio" typed as a + U+0301 combining acute.
        let text = "a\u{301}gio de R$ 80 mil";
        let f = FoldedText::new(text);
        assert_eq!(f.as_str(), "agio de r$ 80 mil");
        let ev = f.evidence(0..4);
        assert_eq!(ev.snippet, "a\u{301}gio");
    }
}
