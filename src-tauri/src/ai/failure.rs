//! Why an AI prioritisation did not produce a result. Each case is reported to the interface as
//! its own `kind`; none of them touches the local ranking.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AiFailure {
    #[error("AI prioritisation is turned off in the settings")]
    Disabled,
    #[error("no API key is stored")]
    NoKey,
    #[error("there is nothing to prioritise: no property has a local score yet")]
    NoCandidates,
    #[error("another AI request is already running")]
    Busy,
    #[error("the AI request was cancelled")]
    Cancelled,
    /// HTTP 429. Never retried automatically: retrying a quota error only burns more of it.
    #[error("the provider's quota or rate limit was reached{}", retry_hint(*.retry_after))]
    Quota { retry_after: Option<Duration> },
    /// HTTP 401 or 403.
    #[error("the provider did not accept the API key (HTTP {status})")]
    Unauthorized { status: u16 },
    /// Any other 4xx: the request itself was refused (for example an unknown model name).
    #[error("the provider refused the request (HTTP {status}): {message}")]
    RequestRefused { status: u16, message: String },
    #[error("the provider did not answer in time")]
    Timeout,
    #[error("the provider could not be reached: {0}")]
    Network(String),
    #[error("the provider reported an error (HTTP {status})")]
    Server { status: u16 },
    /// The provider declined to answer (safety or policy block).
    #[error("the provider blocked the request: {0}")]
    Blocked(String),
    /// The answer was cut short by the output limit.
    #[error("the answer was cut off before it was complete")]
    Truncated,
    /// The answer is not the JSON shape that was asked for.
    #[error("the answer could not be read: {0}")]
    Malformed(String),
    /// The answer was well-formed but breaks a rule (unknown or missing property, bad ranking).
    #[error("the answer was rejected: {0}")]
    OutputRejected(String),
}

fn retry_hint(retry_after: Option<Duration>) -> String {
    retry_after.map_or_else(String::new, |wait| {
        format!(" (try again in about {} s)", wait.as_secs().max(1))
    })
}

impl AiFailure {
    /// Short machine-readable discriminator for the interface.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Disabled => "ai_disabled",
            Self::NoKey => "ai_no_key",
            Self::NoCandidates => "ai_no_candidates",
            Self::Busy => "ai_busy",
            Self::Cancelled => "ai_cancelled",
            Self::Quota { .. } => "ai_quota",
            Self::Unauthorized { .. } => "ai_unauthorized",
            Self::RequestRefused { .. } => "ai_request_refused",
            Self::Timeout => "ai_timeout",
            Self::Network(_) => "ai_network",
            Self::Server { .. } => "ai_server_error",
            Self::Blocked(_) => "ai_blocked",
            Self::Truncated => "ai_truncated",
            Self::Malformed(_) => "ai_malformed",
            Self::OutputRejected(_) => "ai_output_rejected",
        }
    }
}

#[cfg(test)]
mod tests;
