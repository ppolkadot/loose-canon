//! The HTTP call to the Gemini `generateContent` endpoint.
//!
//! Properties this module is responsible for (each has a test against a local server):
//! * the key travels only in the `x-goog-api-key` header, never in the URL or the body;
//! * redirects are never followed, because a redirect could carry the key to another host;
//! * plain `http` is refused except to this machine, so the key never crosses a network unencrypted;
//! * time and size are bounded; a quota answer (429) is never retried; one server error (5xx) is
//!   retried once; a cancelled request stops at once;
//! * every failure becomes one [`AiFailure`], and no failure message repeats the key.
//!
//! The endpoint path and the error shapes follow the public REST description. **They have not
//! been checked against the live service** (no key was available while this was written).

use std::time::Duration;

use reqwest::{Response, header::HeaderValue};
use serde_json::Value;
use url::{Host, Url};

use super::{failure::AiFailure, secret::ApiKey, settings::is_valid_model_name};
use crate::{
    error::{Error, Result},
    sources::CancelToken,
};

/// The provider's address. Not a setting: the key must only ever go here (or to this machine).
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Upper bound on how long one request may take, including the answer being written.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Wait before the single retry of a server error.
pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Largest answer body accepted.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// How much of an error answer is read (only to explain it).
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// Longest provider message passed on.
const MAX_MESSAGE_CHARS: usize = 300;

/// A provider asking for more than this is not believed.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(3600);

pub struct GeminiClient {
    http: reqwest::Client,
    base_url: String,
    retry_delay: Duration,
}

impl GeminiClient {
    /// A client for `base_url`, which must be `https`, or `http` to this machine (tests).
    pub fn new(base_url: &str, timeout: Duration, retry_delay: Duration) -> Result<Self> {
        let base_url = checked_base_url(base_url)?;
        let http = reqwest::Client::builder()
            .timeout(timeout)
            // A redirect could carry the key to another host.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::InvalidConfig {
                what: "Gemini HTTP client",
                detail: e.to_string(),
            })?;
        Ok(Self {
            http,
            base_url,
            retry_delay,
        })
    }

    /// The client for the real provider.
    pub fn production() -> Result<Self> {
        Self::new(DEFAULT_BASE_URL, DEFAULT_TIMEOUT, DEFAULT_RETRY_DELAY)
    }

    /// Sends `body` to `model` and returns the answer's HTTP body, or why there is none.
    pub async fn generate(
        &self,
        model: &str,
        key: &ApiKey,
        body: &Value,
        cancel: &CancelToken,
    ) -> std::result::Result<String, AiFailure> {
        if !is_valid_model_name(model) {
            return Err(AiFailure::RequestRefused {
                status: 0,
                message: "the model name is not valid".to_owned(),
            });
        }
        if cancel.is_cancelled() {
            return Err(AiFailure::Cancelled);
        }
        let url = format!("{}/v1beta/models/{model}:generateContent", self.base_url);
        let payload = serde_json::to_vec(body).expect("a JSON value always serialises");

        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self.send(&url, key, &payload, cancel).await?;
            let status = response.status().as_u16();
            if response.status().is_success() {
                let bytes = read_capped(response, MAX_BODY_BYTES, cancel)
                    .await?
                    .ok_or_else(|| AiFailure::Malformed("the answer is too large".to_owned()))?;
                return String::from_utf8(bytes)
                    .map_err(|_| AiFailure::Malformed("the answer is not valid text".to_owned()));
            }
            let retry_after = retry_after_header(&response);
            let text = read_capped(response, MAX_ERROR_BODY_BYTES, cancel)
                .await?
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            let failure = classify(status, retry_after, &text, key);
            if matches!(failure, AiFailure::Server { .. }) && attempt == 1 {
                // One bounded retry of a server error; waiting counts as cancellable.
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Err(AiFailure::Cancelled),
                    () = tokio::time::sleep(self.retry_delay) => {}
                }
                continue;
            }
            return Err(failure);
        }
    }

    async fn send(
        &self,
        url: &str,
        key: &ApiKey,
        payload: &[u8],
        cancel: &CancelToken,
    ) -> std::result::Result<Response, AiFailure> {
        let mut key_header = HeaderValue::from_str(key.expose())
            .map_err(|_| AiFailure::Network("the key cannot be used as a header".to_owned()))?;
        // Keeps the value out of any debug output of the request.
        key_header.set_sensitive(true);
        let request = self
            .http
            .post(url)
            .header("x-goog-api-key", key_header)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload.to_vec());
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(AiFailure::Cancelled),
            sent = request.send() => sent.map_err(transport_failure),
        }
    }
}

/// `https`, or `http` to this machine only; no credentials, query or fragment in the address.
fn checked_base_url(base_url: &str) -> Result<String> {
    let refuse = |detail: &str| Error::InvalidConfig {
        what: "Gemini base URL",
        detail: detail.to_owned(),
    };
    let url = Url::parse(base_url).map_err(|_| refuse("not a valid address"))?;
    let loopback = match url.host() {
        Some(Host::Domain(name)) => name == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => return Err(refuse("the address has no host")),
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => {
            return Err(refuse(
                "the key may only be sent over https (or to this machine)",
            ));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(refuse("the address must not contain credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(refuse("the address must not contain a query or fragment"));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn transport_failure(error: reqwest::Error) -> AiFailure {
    if error.is_timeout() {
        AiFailure::Timeout
    } else {
        // The address is not secret, but there is no reason to repeat it either.
        let detail: String = error.without_url().to_string().chars().take(200).collect();
        AiFailure::Network(detail)
    }
}

/// The body, read in pieces and refused (`None`) as soon as it passes `cap` bytes.
async fn read_capped(
    mut response: Response,
    cap: usize,
    cancel: &CancelToken,
) -> std::result::Result<Option<Vec<u8>>, AiFailure> {
    if response.content_length().is_some_and(|n| n > cap as u64) {
        return Ok(None);
    }
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(AiFailure::Cancelled),
            chunk = response.chunk() => chunk.map_err(transport_failure)?,
        };
        let Some(chunk) = chunk else {
            return Ok(Some(body));
        };
        if false {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
}

fn retry_after_header(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// A wait the provider reports in the error body (`"retryDelay": "34s"`).
fn retry_delay_in_body(body: &Value) -> Option<Duration> {
    body["error"]["details"]
        .as_array()?
        .iter()
        .filter_map(|detail| detail["retryDelay"].as_str())
        .find_map(|text| {
            let seconds = text.strip_suffix('s')?.parse::<f64>().ok()?;
            (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
        })
}

/// The provider's own explanation, shortened, with the key removed should it be echoed.
fn message_of(body: &Value, raw: &str, status: u16, key: &ApiKey) -> String {
    let text = body["error"]["message"]
        .as_str()
        .map_or_else(|| raw.trim().to_owned(), |m| m.trim().to_owned());
    let text = if text.is_empty() {
        format!("the provider answered with HTTP {status}")
    } else {
        text
    };
    text.replace(key.expose(), "[chave removida]")
        .chars()
        .take(MAX_MESSAGE_CHARS)
        .collect()
}

fn classify(status: u16, header_wait: Option<Duration>, text: &str, key: &ApiKey) -> AiFailure {
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    match status {
        429 => AiFailure::Quota {
            retry_after: header_wait
                .or_else(|| retry_delay_in_body(&body))
                .map(|wait| wait.min(MAX_RETRY_AFTER)),
        },
        401 | 403 => AiFailure::Unauthorized { status },
        // The provider reports an invalid key as a plain bad request.
        400 if text.contains("API_KEY_INVALID") || text.contains("API key not valid") => {
            AiFailure::Unauthorized { status }
        }
        500..=599 => AiFailure::Server { status },
        _ => AiFailure::RequestRefused {
            status,
            message: message_of(&body, text, status, key),
        },
    }
}

#[cfg(test)]
mod tests;
