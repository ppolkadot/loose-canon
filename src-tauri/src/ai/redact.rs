//! Removing contact data from advertisement text before it leaves the machine.
//!
//! The patterns err towards removing: a reference number lost to a phone pattern costs nothing,
//! a phone number sent to a third party cannot be taken back. Money is the one thing protected
//! from the phone pattern (`R$ 12000000` is eight digits but a price), and the tests pin that
//! prices, areas and room counts come through unchanged.

use std::{borrow::Cow, sync::LazyLock};

use regex::{Captures, Regex};

const MARK: &str = "[removido]";

macro_rules! pattern {
    ($pattern:expr) => {{
        static RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new($pattern).expect("valid static regex"));
        &*RE
    }};
}

/// Replaces phone numbers, e-mail addresses, links, social handles and personal or business
/// registration numbers with a neutral marker. Prices, areas and room counts are left alone.
#[must_use]
pub fn redact(text: &str) -> String {
    // Order matters: an address contains a domain, so it goes before the bare-domain pattern.
    let mut out = Cow::Borrowed(text);
    for re in [
        pattern!(r"(?i)[a-z0-9._%+-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)+"),
        pattern!(r"(?i)\b(?:https?://|www\.)\S+"),
        pattern!(
            r"(?i)\b[a-z0-9][a-z0-9-]*(?:\.[a-z0-9-]+)*\.(?:com|net|org|br|io|app|me|info|biz)\b(?:/\S*)?"
        ),
        pattern!(r"\b\d{2}\.\d{3}\.\d{3}/\d{4}-\d{2}\b"),
        pattern!(r"\b\d{3}\.\d{3}\.\d{3}-\d{2}\b"),
        pattern!(r"(?i)\bcreci(?:[-\s]*[a-z]{2}\b)?[\s:.-]*\d[0-9a-z./-]*(?:\s+[a-z]{1,2}\b)?"),
    ] {
        out = Cow::Owned(re.replace_all(&out, MARK).into_owned());
    }
    // `@name` handles keep the character before them (there is no look-behind in this engine).
    out = Cow::Owned(
        pattern!(r"(^|[^\w@])@\w[\w.]*")
            .replace_all(&out, |caps: &Captures<'_>| format!("{}{MARK}", &caps[1]))
            .into_owned(),
    );
    out = Cow::Owned(
        pattern!(r"(?i)(R\$\s*)?(?:0800[\s.-]*\d{3}[\s.-]*\d{4}|(?:\+?55[\s.-]*)?(?:\(?\d{2}\)?[\s.-]*)?(?:9[\s.-]*)?\d{4}[\s.-]?\d{4})")
            .replace_all(&out, |caps: &Captures<'_>| {
                // Digits right after "R$" are an amount, not a phone number.
                if caps.get(1).is_some() {
                    caps[0].to_owned()
                } else {
                    MARK.to_owned()
                }
            })
            .into_owned(),
    );
    out.into_owned()
}

/// The text with contact data removed, whitespace collapsed, and at most `max_chars` characters
/// (cut at a word boundary, with an ellipsis when something was left out).
#[must_use]
pub fn excerpt(text: &str, max_chars: usize) -> String {
    let clean = redact(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if clean.chars().count() <= max_chars {
        return clean;
    }
    let Some(budget) = max_chars.checked_sub(1) else {
        return String::new();
    };
    // Byte offset of the first character that does not fit.
    let cut = clean
        .char_indices()
        .nth(budget)
        .map_or(clean.len(), |(i, _)| i);
    let kept = &clean[..cut];
    let word_break = clean[cut..].starts_with(' ');
    let kept = if word_break {
        kept
    } else {
        // Back up to the last space so no word is cut in half; a single long word is cut hard.
        kept.rfind(' ').map_or(kept, |i| &kept[..i])
    };
    format!("{kept}…")
}

#[cfg(test)]
mod tests;
