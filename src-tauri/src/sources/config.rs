use std::time::Duration;

use crate::{
    error::{Error, Result},
    models::confidence::Confidence,
};

/// Everything that bounds how one source may be used. The defaults are conservative (about one
/// request per second, two at a time, small responses); an adapter may only tighten them when a
/// provider's published limits are stricter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceConfig {
    /// Hosts (exact, or any subdomain) the adapter may contact. Also bounds redirects.
    pub allowed_hosts: Vec<String>,
    pub user_agent: String,
    /// Simultaneous requests to this source.
    pub max_concurrency: usize,
    /// Minimum time between the *starts* of two requests to this source.
    pub min_request_interval: Duration,
    pub request_timeout: Duration,
    pub connect_timeout: Duration,
    /// Extra attempts after the first, for transient failures only.
    pub max_retries: u32,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub max_response_bytes: usize,
    /// Largest image downloaded for this source (separate from the page limit).
    pub max_image_bytes: usize,
    /// Images registered per listing.
    pub max_images_per_listing: usize,
    pub max_redirects: usize,
    /// How long a successful response is reused. `Duration::ZERO` disables the cache.
    pub cache_ttl: Duration,
    pub cache_max_entries: usize,
    /// Total bytes the cache may hold (oldest entries are evicted first).
    pub cache_max_bytes: usize,
    /// Extra per-source tracking parameters to strip when normalising URLs.
    pub ignored_url_params: Vec<String>,
    /// Confidence that an unlabelled amount in this source's structured price field is really the
    /// full asking price. This is a per-source **calibration result**: until the source has been
    /// measured against labelled real listings it must stay `Low`, which keeps its prices out of
    /// comparisons and out of the full-price-only filter.
    pub price_field_confidence: Confidence,
    /// Hard cap on listings fetched from this source in one run.
    pub max_listings_per_run: usize,
}

fn invalid(detail: impl Into<String>) -> Error {
    Error::InvalidConfig {
        what: "source configuration",
        detail: detail.into(),
    }
}

impl SourceConfig {
    /// Conservative defaults for the given hosts.
    #[must_use]
    pub fn new(allowed_hosts: Vec<String>) -> Self {
        Self {
            allowed_hosts,
            user_agent: format!(
                "LooseCanon/{} (local desktop application)",
                env!("CARGO_PKG_VERSION")
            ),
            max_concurrency: 2,
            min_request_interval: Duration::from_secs(1),
            request_timeout: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(10),
            max_retries: 2,
            backoff_base: Duration::from_millis(500),
            backoff_max: Duration::from_secs(8),
            max_response_bytes: 2 * 1024 * 1024,
            max_image_bytes: 8 * 1024 * 1024,
            max_images_per_listing: 6,
            max_redirects: 5,
            cache_ttl: Duration::from_secs(600),
            cache_max_entries: 256,
            cache_max_bytes: 32 * 1024 * 1024,
            ignored_url_params: Vec::new(),
            price_field_confidence: Confidence::Low,
            max_listings_per_run: 200,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.allowed_hosts.is_empty() {
            return Err(invalid("allowed_hosts is empty"));
        }
        for host in &self.allowed_hosts {
            let ok = !host.is_empty()
                && host.len() <= 253
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
                && !host.starts_with(['.', '-'])
                && !host.ends_with(['.', '-']);
            if !ok {
                return Err(invalid(format!(
                    "allowed host {host:?} must be a bare host name"
                )));
            }
        }
        if self.user_agent.trim().is_empty() || self.user_agent.contains(['\r', '\n']) {
            return Err(invalid("user_agent must be a non-empty single line"));
        }
        if !(1..=8).contains(&self.max_concurrency) {
            return Err(invalid("max_concurrency must be between 1 and 8"));
        }
        if self.request_timeout.is_zero() || self.connect_timeout.is_zero() {
            return Err(invalid("timeouts must be greater than zero"));
        }
        if self.max_retries > 5 {
            return Err(invalid(
                "max_retries must not exceed 5 (retries are bounded)",
            ));
        }
        if self.backoff_base.is_zero() || self.backoff_base > self.backoff_max {
            return Err(invalid(
                "backoff_base must be positive and not above backoff_max",
            ));
        }
        if !(1024..=64 * 1024 * 1024).contains(&self.max_response_bytes) {
            return Err(invalid(
                "max_response_bytes must be between 1 KiB and 64 MiB",
            ));
        }
        if !(1024..=64 * 1024 * 1024).contains(&self.max_image_bytes) {
            return Err(invalid("max_image_bytes must be between 1 KiB and 64 MiB"));
        }
        if !(1..=30).contains(&self.max_images_per_listing) {
            return Err(invalid("max_images_per_listing must be between 1 and 30"));
        }
        if self.max_redirects > 10 {
            return Err(invalid("max_redirects must not exceed 10"));
        }
        if !self.cache_ttl.is_zero() && !(1..=10_000).contains(&self.cache_max_entries) {
            return Err(invalid(
                "cache_max_entries must be between 1 and 10000 when the cache is on",
            ));
        }
        if !self.cache_ttl.is_zero() && !(1024..=256 * 1024 * 1024).contains(&self.cache_max_bytes)
        {
            return Err(invalid(
                "cache_max_bytes must be between 1 KiB and 256 MiB when the cache is on",
            ));
        }
        if !(1..=5_000).contains(&self.max_listings_per_run) {
            return Err(invalid("max_listings_per_run must be between 1 and 5000"));
        }
        Ok(())
    }

    /// Whether `host` is one of the allowed hosts or a subdomain of one.
    #[must_use]
    pub fn host_allowed(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.allowed_hosts.iter().any(|allowed| {
            let allowed = allowed.to_ascii_lowercase();
            host == allowed || host.ends_with(&format!(".{allowed}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SourceConfig {
        SourceConfig::new(vec!["example.test".into()])
    }

    #[test]
    fn defaults_are_valid_conservative_and_uncalibrated() {
        let c = config();
        c.validate().unwrap();
        assert_eq!(
            c.price_field_confidence,
            Confidence::Low,
            "uncalibrated sources must stay Low"
        );
        assert!(c.min_request_interval >= Duration::from_secs(1));
        assert!(c.max_concurrency <= 2);
        assert!(c.user_agent.starts_with("LooseCanon/"));
    }

    #[test]
    fn every_bound_is_enforced() {
        type Mutation = Box<dyn Fn(&mut SourceConfig)>;
        let cases: Vec<(&str, Mutation)> = vec![
            ("no hosts", Box::new(|c| c.allowed_hosts.clear())),
            (
                "host with scheme",
                Box::new(|c| c.allowed_hosts = vec!["https://example.test".into()]),
            ),
            (
                "host with path",
                Box::new(|c| c.allowed_hosts = vec!["example.test/x".into()]),
            ),
            (
                "empty host",
                Box::new(|c| c.allowed_hosts = vec![String::new()]),
            ),
            (
                "leading dot",
                Box::new(|c| c.allowed_hosts = vec![".example.test".into()]),
            ),
            ("empty user agent", Box::new(|c| c.user_agent = "  ".into())),
            (
                "multi-line user agent",
                Box::new(|c| c.user_agent = "a\nb".into()),
            ),
            ("zero concurrency", Box::new(|c| c.max_concurrency = 0)),
            ("huge concurrency", Box::new(|c| c.max_concurrency = 9)),
            (
                "zero request timeout",
                Box::new(|c| c.request_timeout = Duration::ZERO),
            ),
            (
                "zero connect timeout",
                Box::new(|c| c.connect_timeout = Duration::ZERO),
            ),
            ("unbounded retries", Box::new(|c| c.max_retries = 6)),
            (
                "zero backoff",
                Box::new(|c| c.backoff_base = Duration::ZERO),
            ),
            (
                "backoff above max",
                Box::new(|c| c.backoff_base = c.backoff_max + Duration::from_secs(1)),
            ),
            (
                "tiny response limit",
                Box::new(|c| c.max_response_bytes = 10),
            ),
            (
                "huge response limit",
                Box::new(|c| c.max_response_bytes = 1 << 30),
            ),
            ("tiny image limit", Box::new(|c| c.max_image_bytes = 10)),
            (
                "huge image limit",
                Box::new(|c| c.max_image_bytes = 1 << 30),
            ),
            (
                "no images per listing",
                Box::new(|c| c.max_images_per_listing = 0),
            ),
            (
                "too many images per listing",
                Box::new(|c| c.max_images_per_listing = 31),
            ),
            ("many redirects", Box::new(|c| c.max_redirects = 11)),
            (
                "cache on with no room",
                Box::new(|c| c.cache_max_entries = 0),
            ),
            (
                "cache with no byte budget",
                Box::new(|c| c.cache_max_bytes = 10),
            ),
            ("zero listing cap", Box::new(|c| c.max_listings_per_run = 0)),
            (
                "huge listing cap",
                Box::new(|c| c.max_listings_per_run = 5_001),
            ),
        ];
        for (name, mutate) in cases {
            let mut c = config();
            mutate(&mut c);
            assert!(
                matches!(c.validate(), Err(Error::InvalidConfig { .. })),
                "{name} must be rejected"
            );
        }
    }

    #[test]
    fn a_disabled_cache_needs_no_capacity() {
        let mut c = config();
        c.cache_ttl = Duration::ZERO;
        c.cache_max_entries = 0;
        c.validate().unwrap();
    }

    #[test]
    fn hosts_match_exactly_or_as_subdomains_only() {
        let c = config();
        assert!(c.host_allowed("example.test"));
        assert!(c.host_allowed("WWW.Example.Test"));
        assert!(c.host_allowed("img.cdn.example.test"));
        assert!(
            !c.host_allowed("evilexample.test"),
            "suffix without a dot boundary"
        );
        assert!(!c.host_allowed("example.test.evil.com"));
        assert!(!c.host_allowed("other.test"));
        assert!(!c.host_allowed(""));
    }
}
