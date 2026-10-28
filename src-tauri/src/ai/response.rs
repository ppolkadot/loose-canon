//! Reading and validating the provider's answer before anything from it is shown.
//!
//! The rule is all-or-nothing, with one stated exception. The answer must be the exact JSON that
//! was asked for, cover exactly the properties that were sent (no unknown, duplicate or missing
//! id), rank them 1..n, and keep every text within bounds; otherwise the whole answer is rejected
//! and the local ranking stands. The exception: a positive claim whose quoted evidence does not
//! occur in the text that was sent is dropped and reported as a warning, because an unsupported
//! claim must never be displayed but one bad quote should not discard an otherwise valid ranking.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::{candidate::Candidate, failure::AiFailure};
use crate::parser::fold_text;

/// Bounds on what the model may write. Exceeding one rejects the answer: cutting the text would
/// alter what the model said.
pub const MAX_EXPLANATION_CHARS: usize = 1_200;
pub const MAX_ITEM_CHARS: usize = 500;
pub const MAX_ITEMS: usize = 10;

/// A strength the model states, with the words of the advertisement it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Positive {
    pub claim: String,
    pub evidence: String,
}

/// What the model said about one property.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyNote {
    pub property_id: i64,
    /// 1 is the property the model would look at first.
    pub ai_rank: u32,
    pub explanation: String,
    pub positives: Vec<Positive>,
    pub concerns: Vec<String>,
    pub uncertainties: Vec<String>,
}

/// An answer that passed validation: notes ordered by the model's rank, and what was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated {
    pub notes: Vec<PropertyNote>,
    pub warnings: Vec<String>,
}

/// Shortest quote that can count as evidence.
const MIN_EVIDENCE_CHARS: usize = 3;

/// What the provider wraps the answer in. Unknown fields are fine here: the provider adds them.
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    candidates: Vec<EnvelopeCandidate>,
    #[serde(default, rename = "promptFeedback")]
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Deserialize)]
struct PromptFeedback {
    #[serde(default, rename = "blockReason")]
    block_reason: Option<String>,
}

#[derive(Deserialize)]
struct EnvelopeCandidate {
    #[serde(default)]
    content: Option<Content>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    #[serde(default)]
    text: Option<String>,
    /// A part holding the model's reasoning, not its answer.
    #[serde(default)]
    thought: bool,
}

/// The answer itself, exactly as the schema asked for it. Anything else is malformed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnswer {
    ranking: Vec<RawItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawItem {
    id: i64,
    rank: i64,
    explanation: String,
    positives: Vec<Positive>,
    concerns: Vec<String>,
    uncertainties: Vec<String>,
}

fn malformed(detail: impl AsRef<str>) -> AiFailure {
    let detail: String = detail.as_ref().chars().take(200).collect();
    AiFailure::Malformed(detail)
}

fn rejected(detail: impl Into<String>) -> AiFailure {
    AiFailure::OutputRejected(detail.into())
}

/// The model's answer text out of the provider's envelope, or why there is none.
fn answer_text(body: &str) -> Result<String, AiFailure> {
    let envelope: Envelope = serde_json::from_str(body)
        .map_err(|e| malformed(format!("the response is not the expected JSON: {e}")))?;
    let Some(first) = envelope.candidates.into_iter().next() else {
        return Err(
            match envelope.prompt_feedback.and_then(|f| f.block_reason) {
                Some(reason) => AiFailure::Blocked(format!("the prompt was blocked ({reason})")),
                None => malformed("the response has no candidates"),
            },
        );
    };
    match first.finish_reason.as_deref() {
        Some("STOP") => {}
        Some("MAX_TOKENS") => return Err(AiFailure::Truncated),
        Some(
            reason @ ("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII"
            | "IMAGE_SAFETY"),
        ) => {
            return Err(AiFailure::Blocked(format!(
                "the answer was blocked ({reason})"
            )));
        }
        Some(other) => return Err(malformed(format!("the answer ended with {other}"))),
        None => return Err(malformed("the answer has no finish reason")),
    }
    let text: String = first
        .content
        .into_iter()
        .flat_map(|c| c.parts)
        .filter(|p| !p.thought)
        .filter_map(|p| p.text)
        .collect();
    if text.trim().is_empty() {
        return Err(malformed("the answer is empty"));
    }
    Ok(text)
}

/// A string the model wrote: not blank, within `max` characters. Returned trimmed.
fn bounded(what: &str, text: &str, max: usize) -> Result<String, AiFailure> {
    let text = text.trim();
    if text.is_empty() {
        return Err(rejected(format!("{what} is empty")));
    }
    if text.chars().count() > max {
        return Err(rejected(format!("{what} is longer than {max} characters")));
    }
    Ok(text.to_owned())
}

fn bounded_list(what: &str, items: &[String]) -> Result<Vec<String>, AiFailure> {
    if items.len() > MAX_ITEMS {
        return Err(rejected(format!("{what} has more than {MAX_ITEMS} items")));
    }
    items
        .iter()
        .map(|item| bounded(what, item, MAX_ITEM_CHARS))
        .collect()
}

/// Folded, with runs of whitespace collapsed: the form quotes are compared in.
fn comparable(text: &str) -> String {
    fold_text(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `quote` occurs in `text` (both in comparable form) as whole words: a fragment from the
/// middle of a word is not a quotation.
fn occurs_as_words(text: &str, quote: &str) -> bool {
    if quote.chars().count() < MIN_EVIDENCE_CHARS {
        return false;
    }
    let before_ok = |at: usize| {
        !quote.starts_with(char::is_alphanumeric) || !text[..at].ends_with(char::is_alphanumeric)
    };
    let after_ok = |end: usize| {
        !quote.ends_with(char::is_alphanumeric) || !text[end..].starts_with(char::is_alphanumeric)
    };
    text.match_indices(quote)
        .any(|(at, found)| before_ok(at) && after_ok(at + found.len()))
}

/// Reads the HTTP body of a `generateContent` answer and validates it against the candidates that
/// were sent.
pub fn parse_answer(body: &str, candidates: &[Candidate]) -> Result<Validated, AiFailure> {
    let text = answer_text(body)?;
    let answer: RawAnswer = serde_json::from_str(&text)
        .map_err(|e| malformed(format!("the answer is not the requested JSON: {e}")))?;

    let sent: HashMap<i64, &Candidate> = candidates.iter().map(|c| (c.id, c)).collect();
    let n = candidates.len();
    let mut seen = BTreeSet::new();
    for item in &answer.ranking {
        if !sent.contains_key(&item.id) {
            return Err(rejected(format!(
                "property {} was not among those sent",
                item.id
            )));
        }
        if !seen.insert(item.id) {
            return Err(rejected(format!(
                "property {} appears more than once",
                item.id
            )));
        }
    }
    if let Some(missing) = candidates.iter().find(|c| !seen.contains(&c.id)) {
        return Err(rejected(format!("property {} is missing", missing.id)));
    }
    let ranks: BTreeSet<i64> = answer.ranking.iter().map(|i| i.rank).collect();
    let expected: BTreeSet<i64> = (1..=i64::try_from(n).unwrap_or(i64::MAX)).collect();
    if ranks != expected {
        return Err(rejected(format!(
            "the ranks are not 1 to {n} without repeats or gaps"
        )));
    }

    let mut notes = Vec::with_capacity(n);
    let mut warnings = Vec::new();
    for item in &answer.ranking {
        let own_text = comparable(&sent[&item.id].text);
        let mut positives = Vec::new();
        if item.positives.len() > MAX_ITEMS {
            return Err(rejected(format!(
                "the positives have more than {MAX_ITEMS} items"
            )));
        }
        for positive in &item.positives {
            let claim = bounded("a positive claim", &positive.claim, MAX_ITEM_CHARS)?;
            if positive.evidence.chars().count() > MAX_ITEM_CHARS {
                return Err(rejected(format!(
                    "a quoted evidence is longer than {MAX_ITEM_CHARS} characters"
                )));
            }
            let quote = comparable(&positive.evidence);
            if occurs_as_words(&own_text, &quote) {
                positives.push(Positive {
                    claim,
                    evidence: positive.evidence.trim().to_owned(),
                });
            } else {
                warnings.push(format!(
                    "imóvel {}: a afirmação \"{claim}\" foi descartada porque o trecho citado não aparece no texto enviado",
                    item.id
                ));
            }
        }
        notes.push(PropertyNote {
            property_id: item.id,
            ai_rank: u32::try_from(item.rank).expect("validated to be within 1..=n"),
            explanation: bounded("an explanation", &item.explanation, MAX_EXPLANATION_CHARS)?,
            positives,
            concerns: bounded_list("the concerns", &item.concerns)?,
            uncertainties: bounded_list("the uncertainties", &item.uncertainties)?,
        });
    }
    notes.sort_by_key(|note| note.ai_rank);
    Ok(Validated { notes, warnings })
}

#[cfg(test)]
mod tests;
