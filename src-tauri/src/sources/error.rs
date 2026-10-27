/// Why talking to a source failed. Each variant says whether trying again could help; refusals
/// (`Blocked`) and rate limits are never retried automatically and are never "worked around".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("the source refused the request (HTTP {status}); not retried and not bypassed")]
    Blocked { status: u16 },
    #[error("not found (HTTP {status})")]
    NotFound { status: u16 },
    #[error("rate limited by the source (HTTP 429), retry after {retry_after_secs:?} s")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("the request timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("server error (HTTP {status})")]
    Server { status: u16 },
    #[error("unexpected HTTP status {status}")]
    UnexpectedStatus { status: u16 },
    #[error("the response is larger than the {limit} byte limit")]
    TooLarge { limit: usize },
    #[error("URL not allowed: {0}")]
    Disallowed(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("content could not be parsed reliably: {0}")]
    Unparseable(String),
    #[error("cancelled")]
    Cancelled,
    #[error("source error: {0}")]
    Other(String),
}

impl SourceError {
    /// Stable machine-readable name, stored as `source_run_outcomes.error_kind`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Blocked { .. } => "blocked",
            Self::NotFound { .. } => "not_found",
            Self::RateLimited { .. } => "rate_limited",
            Self::Timeout => "timeout",
            Self::Network(_) => "network",
            Self::Server { .. } => "server_error",
            Self::UnexpectedStatus { .. } => "unexpected_status",
            Self::TooLarge { .. } => "too_large",
            Self::Disallowed(_) => "disallowed",
            Self::TooManyRedirects => "too_many_redirects",
            Self::Unparseable(_) => "unparseable",
            Self::Cancelled => "cancelled",
            Self::Other(_) => "other",
        }
    }

    /// Only transient failures are retried, and only a bounded number of times.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Timeout | Self::Network(_) | Self::Server { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_transient_failures_are_retryable() {
        for e in [
            SourceError::Timeout,
            SourceError::Network("x".into()),
            SourceError::Server { status: 503 },
        ] {
            assert!(e.is_retryable(), "{e}");
        }
        for e in [
            SourceError::Blocked { status: 403 },
            SourceError::NotFound { status: 404 },
            SourceError::RateLimited {
                retry_after_secs: Some(5),
            },
            SourceError::UnexpectedStatus { status: 418 },
            SourceError::TooLarge { limit: 1 },
            SourceError::Disallowed("x".into()),
            SourceError::TooManyRedirects,
            SourceError::Unparseable("x".into()),
            SourceError::Cancelled,
            SourceError::Other("x".into()),
        ] {
            assert!(!e.is_retryable(), "{e} must not be retried");
        }
    }

    #[test]
    fn kinds_are_distinct_stable_names() {
        let all = [
            SourceError::Blocked { status: 403 },
            SourceError::NotFound { status: 404 },
            SourceError::RateLimited {
                retry_after_secs: None,
            },
            SourceError::Timeout,
            SourceError::Network(String::new()),
            SourceError::Server { status: 500 },
            SourceError::UnexpectedStatus { status: 418 },
            SourceError::TooLarge { limit: 1 },
            SourceError::Disallowed(String::new()),
            SourceError::TooManyRedirects,
            SourceError::Unparseable(String::new()),
            SourceError::Cancelled,
            SourceError::Other(String::new()),
        ];
        let kinds: std::collections::HashSet<&str> = all.iter().map(SourceError::kind).collect();
        assert_eq!(kinds.len(), all.len());
        assert_eq!(SourceError::Blocked { status: 403 }.kind(), "blocked");
    }
}
