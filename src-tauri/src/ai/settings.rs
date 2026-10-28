//! The user's AI settings. The API key is not one of them: it lives only in the credential store.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Settings key (`app_settings`).
pub const AI_SETTINGS_KEY: &str = "ai_settings";

/// The model used until the user picks another. **Unverified**: this build could not check the
/// provider's current model list, so the name is a setting rather than a promise.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// How many of the best-ranked properties are sent (the specification asks for 15 to 30).
pub const MIN_CANDIDATES: usize = 15;
pub const MAX_CANDIDATES: usize = 30;
const DEFAULT_CANDIDATES: usize = 20;

const MAX_MODEL_LEN: usize = 64;

/// Whether `model` is a plain model id. It becomes part of a URL path, so only letters, digits
/// and `.`, `-`, `_` are accepted, starting with a letter or digit (which also rules out the empty
/// name).
#[must_use]
pub fn is_valid_model_name(model: &str) -> bool {
    model.len() <= MAX_MODEL_LEN
        && model.starts_with(|c: char| c.is_ascii_alphanumeric())
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !model.contains("..")
}

/// Off until the user turns it on: sending data to a third party is opt-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct AiSettings {
    pub enabled: bool,
    pub model: String,
    pub candidate_count: usize,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            model: DEFAULT_MODEL.to_owned(),
            candidate_count: DEFAULT_CANDIDATES,
        }
    }
}

impl AiSettings {
    /// The model name ends up in a URL path, so only plain model-id characters are accepted.
    pub fn validate(&self) -> Result<()> {
        let model_ok = is_valid_model_name(&self.model);
        let problem = if !model_ok {
            Some(format!(
                "the model name must be 1-{MAX_MODEL_LEN} letters, digits, '.', '-' or '_'"
            ))
        } else if !(MIN_CANDIDATES..=MAX_CANDIDATES).contains(&self.candidate_count) {
            Some(format!(
                "the number of candidates must be between {MIN_CANDIDATES} and {MAX_CANDIDATES}"
            ))
        } else {
            None
        };
        problem.map_or(Ok(()), |detail| {
            Err(Error::InvalidConfig {
                what: "AI settings",
                detail,
            })
        })
    }
}

#[cfg(test)]
mod tests;
