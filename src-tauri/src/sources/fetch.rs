//! The bounded, polite HTTP fetcher every adapter goes through.
//!
//! One `Fetcher` per source. It enforces, for every request: a host allow-list (also for
//! redirects), per-source and global concurrency limits, a minimum interval between request
//! starts, timeouts, a bounded number of retries for transient failures only, response size
//! limits counted on the decoded bytes, a bounded cache of successful responses, and
//! cancellation at every wait. A refusal (403...) or a rate limit is reported, never retried and
//! never worked around.

use std::{
    collections::{HashMap, VecDeque},
    error::Error as StdError,
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant as StdInstant},
};

use reqwest::{header, redirect::Policy};
use tokio::{sync::Semaphore, time::Instant};

use super::{CancelToken, SourceConfig, SourceError, url_norm::parse_http_url};
use crate::error::{Error, Result};

/// A fetched page (or cache hit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub requested_url: String,
    /// The URL after redirects.
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub from_cache: bool,
}

impl Fetched {
    /// The body as UTF-8, replacing invalid sequences. Adapters that need other charsets decode
    /// `body` themselves.
    #[must_use]
    pub fn text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Retry-After values above this are clamped when reported.
const MAX_REPORTED_RETRY_AFTER_SECS: u64 = 3_600;

#[derive(Debug)]
enum Rejected {
    Host(String),
    TooMany,
    Downgrade,
}

/// Error raised from inside the redirect policy so [`map_error`] can recognise it in the source
/// chain of the resulting `reqwest::Error`.
#[derive(Debug)]
struct RedirectRejected(Rejected);

impl fmt::Display for RedirectRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Rejected::Host(host) => write!(f, "redirect to {host:?} is not an allowed host"),
            Rejected::TooMany => f.write_str("too many redirects"),
            Rejected::Downgrade => f.write_str("redirect from https to http"),
        }
    }
}

impl StdError for RedirectRejected {}

fn map_error(error: &reqwest::Error) -> SourceError {
    let mut source: Option<&(dyn StdError + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(RedirectRejected(why)) = current.downcast_ref::<RedirectRejected>() {
            return match why {
                Rejected::Host(host) => SourceError::Disallowed(format!("redirect to {host:?}")),
                Rejected::TooMany => SourceError::TooManyRedirects,
                Rejected::Downgrade => {
                    SourceError::Disallowed("redirect from https to http".to_owned())
                }
            };
        }
        source = current.source();
    }
    if error.is_timeout() {
        SourceError::Timeout
    } else if error.is_redirect() {
        SourceError::TooManyRedirects
    } else {
        // Strip URLs and other details from the message: it may be logged or shown.
        SourceError::Network(if error.is_connect() {
            "could not connect".to_owned()
        } else if error.is_body() || error.is_decode() {
            "error while reading the response".to_owned()
        } else {
            "request failed".to_owned()
        })
    }
}

/// Runs `fut`, abandoning it the moment `cancel` fires.
async fn cancellable<T>(
    cancel: &CancelToken,
    fut: impl Future<Output = std::result::Result<T, SourceError>>,
) -> std::result::Result<T, SourceError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(SourceError::Cancelled),
        result = fut => result,
    }
}

/// Spaces out the *starts* of requests. Slots are reserved under a short lock and slept on
/// outside it, so concurrent callers queue up behind each other.
struct RateLimiter {
    interval: Duration,
    next_slot: Mutex<Option<Instant>>,
}

impl RateLimiter {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            next_slot: Mutex::new(None),
        }
    }

    async fn wait(&self, cancel: &CancelToken) -> std::result::Result<(), SourceError> {
        if self.interval.is_zero() {
            return Ok(());
        }
        let slot = {
            let mut next = self.next_slot.lock().expect("rate limiter lock");
            let now = Instant::now();
            let slot = next.map_or(now, |n| n.max(now));
            *next = Some(slot + self.interval);
            slot
        };
        if slot <= Instant::now() {
            return Ok(());
        }
        cancellable(cancel, async {
            tokio::time::sleep_until(slot).await;
            Ok(())
        })
        .await
    }
}

struct CacheEntry {
    fetched: Fetched,
    stored_at: StdInstant,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<String, CacheEntry>,
    order: VecDeque<String>,
    total_bytes: usize,
}

impl Cache {
    fn get(&mut self, key: &str, ttl: Duration) -> Option<Fetched> {
        let fresh = self
            .entries
            .get(key)
            .is_some_and(|entry| entry.stored_at.elapsed() < ttl);
        if !fresh {
            self.remove(key);
            return None;
        }
        self.entries.get(key).map(|entry| Fetched {
            from_cache: true,
            ..entry.fetched.clone()
        })
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.total_bytes -= entry.fetched.body.len();
            self.order.retain(|k| k != key);
        }
    }

    fn put(&mut self, key: String, fetched: Fetched, max_entries: usize, max_bytes: usize) {
        let size = fetched.body.len();
        if size > max_bytes {
            return;
        }
        self.remove(&key);
        while let Some(oldest) = self.order.front().cloned() {
            if self.entries.len() < max_entries && self.total_bytes + size <= max_bytes {
                break;
            }
            self.remove(&oldest);
        }
        self.total_bytes += size;
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            CacheEntry {
                fetched,
                stored_at: StdInstant::now(),
            },
        );
    }
}

pub struct Fetcher {
    client: reqwest::Client,
    config: SourceConfig,
    global: Arc<Semaphore>,
    local: Semaphore,
    limiter: RateLimiter,
    cache: Mutex<Cache>,
}

impl Fetcher {
    /// `global` bounds simultaneous requests across **all** sources.
    pub fn new(config: SourceConfig, global: Arc<Semaphore>) -> Result<Self> {
        config.validate()?;
        let hosts = config.clone();
        let max_redirects = config.max_redirects;
        let policy = Policy::custom(move |attempt| {
            if attempt.previous().len() > max_redirects {
                return attempt.error(RedirectRejected(Rejected::TooMany));
            }
            let downgrade = attempt
                .previous()
                .last()
                .is_some_and(|u| u.scheme() == "https")
                && attempt.url().scheme() != "https";
            if downgrade {
                return attempt.error(RedirectRejected(Rejected::Downgrade));
            }
            let host = attempt.url().host_str().map(str::to_owned);
            match host {
                Some(host) if hosts.host_allowed(&host) => attempt.follow(),
                other => attempt.error(RedirectRejected(Rejected::Host(other.unwrap_or_default()))),
            }
        });
        let client = reqwest::Client::builder()
            .user_agent(config.user_agent.as_str())
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .redirect(policy)
            .build()
            .map_err(|e| Error::InvalidConfig {
                what: "http client",
                detail: e.to_string(),
            })?;
        Ok(Self {
            client,
            local: Semaphore::new(config.max_concurrency),
            limiter: RateLimiter::new(config.min_request_interval),
            cache: Mutex::new(Cache::default()),
            config,
            global,
        })
    }

    #[must_use]
    pub const fn config(&self) -> &SourceConfig {
        &self.config
    }

    /// Fetches a page: bounded by `max_response_bytes`, served from the cache when fresh.
    pub async fn get(
        &self,
        raw_url: &str,
        cancel: &CancelToken,
    ) -> std::result::Result<Fetched, SourceError> {
        self.fetch(raw_url, cancel, self.config.max_response_bytes, true)
            .await
    }

    /// Fetches an image: bounded by `max_image_bytes` and never cached in memory (images are
    /// written to the on-disk cache by the caller). All other limits and politeness rules apply.
    pub async fn get_image(
        &self,
        raw_url: &str,
        cancel: &CancelToken,
    ) -> std::result::Result<Fetched, SourceError> {
        self.fetch(raw_url, cancel, self.config.max_image_bytes, false)
            .await
    }

    async fn fetch(
        &self,
        raw_url: &str,
        cancel: &CancelToken,
        limit: usize,
        use_cache: bool,
    ) -> std::result::Result<Fetched, SourceError> {
        let url = parse_http_url(raw_url).map_err(|e| SourceError::Disallowed(e.to_string()))?;
        let host = url.host_str().unwrap_or_default();
        if !self.config.host_allowed(host) {
            return Err(SourceError::Disallowed(format!(
                "host {host:?} is not allowed for this source"
            )));
        }
        if cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        let key = url.as_str().to_owned();
        let cache_on = use_cache && !self.config.cache_ttl.is_zero();
        if cache_on
            && let Some(hit) = self
                .cache
                .lock()
                .expect("cache lock")
                .get(&key, self.config.cache_ttl)
        {
            return Ok(hit);
        }

        let mut retries = 0_u32;
        loop {
            match self.attempt(&url, cancel, limit).await {
                Ok(fetched) => {
                    if cache_on {
                        self.cache.lock().expect("cache lock").put(
                            key,
                            fetched.clone(),
                            self.config.cache_max_entries,
                            self.config.cache_max_bytes,
                        );
                    }
                    return Ok(fetched);
                }
                Err(error) if error.is_retryable() && retries < self.config.max_retries => {
                    retries += 1;
                    let exponent = (retries - 1).min(16);
                    let delay = self
                        .config
                        .backoff_base
                        .saturating_mul(1 << exponent)
                        .min(self.config.backoff_max);
                    cancellable(cancel, async {
                        tokio::time::sleep(delay).await;
                        Ok(())
                    })
                    .await?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// One attempt: permits, then a rate-limit slot, then the request.
    async fn attempt(
        &self,
        url: &url::Url,
        cancel: &CancelToken,
        limit: usize,
    ) -> std::result::Result<Fetched, SourceError> {
        let closed = || SourceError::Other("concurrency limiter closed".to_owned());
        let _global = cancellable(cancel, async {
            self.global.acquire().await.map_err(|_| closed())
        })
        .await?;
        let _local = cancellable(cancel, async {
            self.local.acquire().await.map_err(|_| closed())
        })
        .await?;
        self.limiter.wait(cancel).await?;

        let mut response = cancellable(cancel, async {
            self.client
                .get(url.clone())
                .send()
                .await
                .map_err(|e| map_error(&e))
        })
        .await?;

        let status = response.status().as_u16();
        match status {
            200..=299 => {}
            401 | 403 | 451 => return Err(SourceError::Blocked { status }),
            404 | 410 => return Err(SourceError::NotFound { status }),
            408 => return Err(SourceError::Timeout),
            429 => {
                let retry_after_secs = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(|s| s.min(MAX_REPORTED_RETRY_AFTER_SECS));
                return Err(SourceError::RateLimited { retry_after_secs });
            }
            500..=599 => return Err(SourceError::Server { status }),
            _ => return Err(SourceError::UnexpectedStatus { status }),
        }

        if response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            return Err(SourceError::TooLarge { limit });
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let final_url = response.url().to_string();

        // Counted on the decoded bytes as they arrive, so a compressed bomb is stopped too.
        let mut body = Vec::new();
        while let Some(chunk) = cancellable(cancel, async {
            response.chunk().await.map_err(|e| map_error(&e))
        })
        .await?
        {
            if body.len() + chunk.len() > limit {
                return Err(SourceError::TooLarge { limit });
            }
            body.extend_from_slice(&chunk);
        }

        Ok(Fetched {
            requested_url: url.to_string(),
            final_url,
            status,
            content_type,
            body,
            from_cache: false,
        })
    }
}

#[cfg(test)]
mod tests;
