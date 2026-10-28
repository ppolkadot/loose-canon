use std::time::Instant;

use serde_json::json;

use super::*;
use crate::sources::testserver::{Reply, TestServer};

const KEY: &str = "AIzaSyTESTKEY-not-a-real-key_0123456789";
const MODEL: &str = "gemini-test";
const PATH: &str = "/v1beta/models/gemini-test:generateContent";

fn key() -> ApiKey {
    ApiKey::parse(KEY).unwrap()
}

fn client_for(server: &TestServer) -> GeminiClient {
    GeminiClient::new(
        &format!("http://127.0.0.1:{}", server.port()),
        Duration::from_secs(5),
        Duration::from_millis(20),
    )
    .unwrap()
}

fn payload() -> Value {
    json!({ "contents": [{ "role": "user", "parts": [{ "text": "olá" }] }] })
}

async fn call(server: &TestServer) -> std::result::Result<String, AiFailure> {
    client_for(server)
        .generate(MODEL, &key(), &payload(), &CancelToken::new())
        .await
}

fn error_body(code: u16, status: &str, message: &str) -> String {
    json!({ "error": { "code": code, "message": message, "status": status } }).to_string()
}

// ---- the request ---------------------------------------------------------------------------

#[tokio::test]
async fn a_good_answer_is_returned_as_its_body_text() {
    let server = TestServer::start(|_| Reply::ok(r#"{"candidates":[]}"#)).await;
    assert_eq!(call(&server).await.unwrap(), r#"{"candidates":[]}"#);
}

#[tokio::test]
async fn the_request_is_a_json_post_to_the_model_path_with_the_key_in_a_header_only() {
    let server = TestServer::start(|_| Reply::ok("{}")).await;
    call(&server).await.unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, PATH);
    assert_eq!(request.header("x-goog-api-key"), Some(KEY));
    assert!(
        request
            .header("content-type")
            .is_some_and(|c| c.starts_with("application/json")),
        "{:?}",
        request.headers
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&request.body).unwrap(),
        payload()
    );
    // The key is nowhere else: not in the address, not in the body, not in another header.
    assert!(!request.path.contains(KEY) && !request.path.contains('?'));
    assert!(!request.body_text().contains(KEY));
    let others = request
        .headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("x-goog-api-key"))
        .filter(|(_, value)| value.contains(KEY))
        .count();
    assert_eq!(others, 0, "{:?}", request.headers);
}

#[tokio::test]
async fn a_model_name_that_could_change_the_address_is_refused_before_anything_is_sent() {
    let server = TestServer::start(|_| Reply::ok("{}")).await;
    for bad in ["", "../v1/other", "a/b", "m?key=x", "m#x", "m n", "m:other"] {
        let result = client_for(&server)
            .generate(bad, &key(), &payload(), &CancelToken::new())
            .await;
        assert!(
            matches!(result, Err(AiFailure::RequestRefused { status: 0, .. })),
            "{bad:?}: {result:?}"
        );
    }
    assert_eq!(server.hits(), 0);
}

// ---- where the key may be sent -------------------------------------------------------------

#[test]
fn the_key_can_only_be_sent_over_https_or_to_this_machine() {
    let build = |url: &str| GeminiClient::new(url, Duration::from_secs(1), Duration::ZERO);
    for ok in [
        "https://generativelanguage.googleapis.com",
        "https://generativelanguage.googleapis.com/",
        "http://127.0.0.1:8080",
        "http://localhost:3000",
        "http://[::1]:9",
    ] {
        build(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in [
        "http://generativelanguage.googleapis.com",
        "http://example.com",
        "http://127.0.0.1.evil.example.com",
        "http://localhost.evil.example.com",
        "ftp://127.0.0.1",
        "file:///etc",
        "not a url",
        "",
        "https://user:pass@generativelanguage.googleapis.com",
        "https://generativelanguage.googleapis.com/?k=v",
    ] {
        let error = build(bad)
            .err()
            .unwrap_or_else(|| panic!("{bad:?} must be refused"));
        assert_eq!(error.kind(), "invalid_config", "{bad:?}");
    }
}

#[tokio::test]
async fn the_production_client_points_at_the_provider() {
    assert!(DEFAULT_BASE_URL.starts_with("https://"));
    GeminiClient::production().unwrap();
}

#[tokio::test]
async fn a_redirect_is_never_followed_so_the_key_cannot_be_forwarded() {
    let other = TestServer::start(|_| Reply::ok("{}")).await;
    let target = other.url("/stolen");
    let server = TestServer::start(move |_| Reply::redirect(&target)).await;
    let result = call(&server).await;
    assert!(
        matches!(result, Err(AiFailure::RequestRefused { status: 302, .. })),
        "{result:?}"
    );
    assert_eq!(
        other.hits(),
        0,
        "the key must not reach the redirect target"
    );
    assert!(
        other
            .requests()
            .iter()
            .all(|r| r.header("x-goog-api-key").is_none())
    );
}

// ---- failures ------------------------------------------------------------------------------

#[tokio::test]
async fn a_quota_answer_is_reported_with_its_wait_and_is_never_retried() {
    let with_header = TestServer::start(|_| {
        Reply::status(429)
            .header("Retry-After", "30")
            .body(error_body(429, "RESOURCE_EXHAUSTED", "quota").as_bytes())
    })
    .await;
    assert_eq!(
        call(&with_header).await,
        Err(AiFailure::Quota {
            retry_after: Some(Duration::from_secs(30))
        })
    );
    assert_eq!(with_header.hits(), 1, "a quota error must not be retried");

    let in_body = TestServer::start(|_| {
        let body = json!({ "error": { "code": 429, "message": "quota", "status": "RESOURCE_EXHAUSTED",
            "details": [{ "@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "12s" }] } });
        Reply::status(429).body(body.to_string().as_bytes())
    })
    .await;
    assert_eq!(
        call(&in_body).await,
        Err(AiFailure::Quota {
            retry_after: Some(Duration::from_secs(12))
        })
    );

    let fractional = TestServer::start(|_| {
        let body = json!({ "error": { "details": [{ "retryDelay": "0.5s" }] } });
        Reply::status(429).body(body.to_string().as_bytes())
    })
    .await;
    assert_eq!(
        call(&fractional).await,
        Err(AiFailure::Quota {
            retry_after: Some(Duration::from_millis(500))
        })
    );

    let none = TestServer::start(|_| Reply::status(429)).await;
    assert_eq!(
        call(&none).await,
        Err(AiFailure::Quota { retry_after: None })
    );
}

#[tokio::test]
async fn an_absurd_wait_is_capped_rather_than_trusted() {
    let server = TestServer::start(|_| Reply::status(429).header("Retry-After", "99999999")).await;
    match call(&server).await {
        Err(AiFailure::Quota {
            retry_after: Some(wait),
        }) => assert!(wait <= Duration::from_secs(3600), "{wait:?}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_rejected_key_is_reported_as_such_whatever_status_the_provider_uses() {
    for status in [401_u16, 403] {
        let server = TestServer::start(move |_| {
            Reply::status(status).body(error_body(status, "PERMISSION_DENIED", "no").as_bytes())
        })
        .await;
        assert_eq!(call(&server).await, Err(AiFailure::Unauthorized { status }));
        assert_eq!(server.hits(), 1);
    }
    let invalid = TestServer::start(|_| {
        let body = json!({ "error": { "code": 400, "message": "API key not valid. Please pass a valid API key.",
            "status": "INVALID_ARGUMENT", "details": [{ "reason": "API_KEY_INVALID" }] } });
        Reply::status(400).body(body.to_string().as_bytes())
    })
    .await;
    assert_eq!(
        call(&invalid).await,
        Err(AiFailure::Unauthorized { status: 400 })
    );
}

#[tokio::test]
async fn other_client_errors_carry_the_providers_own_message() {
    let unknown_model = TestServer::start(|_| {
        Reply::status(404)
            .body(error_body(404, "NOT_FOUND", "models/gemini-test is not found").as_bytes())
    })
    .await;
    match call(&unknown_model).await {
        Err(AiFailure::RequestRefused {
            status: 404,
            message,
        }) => assert!(message.contains("is not found"), "{message}"),
        other => panic!("{other:?}"),
    }
    let bad_request = TestServer::start(|_| {
        Reply::status(400)
            .body(error_body(400, "INVALID_ARGUMENT", "Invalid JSON payload").as_bytes())
    })
    .await;
    match call(&bad_request).await {
        Err(AiFailure::RequestRefused {
            status: 400,
            message,
        }) => assert!(message.contains("Invalid JSON payload"), "{message}"),
        other => panic!("{other:?}"),
    }
    let plain = TestServer::start(|_| Reply::status(418).body(b"short and stout")).await;
    match call(&plain).await {
        Err(AiFailure::RequestRefused {
            status: 418,
            message,
        }) => assert!(message.contains("short and stout"), "{message}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_message_never_repeats_the_key_even_if_the_provider_echoes_it() {
    let server = TestServer::start(|_| {
        Reply::status(400).body(
            error_body(
                400,
                "INVALID_ARGUMENT",
                &format!("bad request for key {KEY}"),
            )
            .as_bytes(),
        )
    })
    .await;
    match call(&server).await {
        Err(failure @ AiFailure::RequestRefused { .. }) => {
            let shown = format!("{failure} {failure:?}");
            assert!(!shown.contains(KEY), "{shown}");
        }
        other => panic!("{other:?}"),
    }
    let long = TestServer::start(|_| Reply::status(400).body("x".repeat(5000).as_bytes())).await;
    match call(&long).await {
        Err(AiFailure::RequestRefused { message, .. }) => {
            assert!(message.chars().count() <= 300, "{}", message.len())
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_server_error_is_retried_once_and_then_reported() {
    let recovers = TestServer::start(|req| {
        if req.hit == 1 {
            Reply::status(503)
        } else {
            Reply::ok(r#"{"ok":true}"#)
        }
    })
    .await;
    assert_eq!(call(&recovers).await.unwrap(), r#"{"ok":true}"#);
    assert_eq!(recovers.hits(), 2);

    let stays_down = TestServer::start(|_| Reply::status(503)).await;
    assert_eq!(
        call(&stays_down).await,
        Err(AiFailure::Server { status: 503 })
    );
    assert_eq!(stays_down.hits(), 2, "exactly one retry");

    let then_quota = TestServer::start(|req| {
        if req.hit == 1 {
            Reply::status(500)
        } else {
            Reply::status(429)
        }
    })
    .await;
    assert_eq!(
        call(&then_quota).await,
        Err(AiFailure::Quota { retry_after: None })
    );
    assert_eq!(then_quota.hits(), 2);
}

#[tokio::test]
async fn a_slow_provider_times_out() {
    let server = TestServer::start(|_| Reply::ok("{}").delay(Duration::from_secs(2))).await;
    let client = GeminiClient::new(
        &format!("http://127.0.0.1:{}", server.port()),
        Duration::from_millis(150),
        Duration::from_millis(10),
    )
    .unwrap();
    let started = Instant::now();
    let result = client
        .generate(MODEL, &key(), &payload(), &CancelToken::new())
        .await;
    assert_eq!(result, Err(AiFailure::Timeout));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(server.hits(), 1, "a timeout is not retried");
}

#[tokio::test]
async fn an_unreachable_provider_is_a_network_failure() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let client = GeminiClient::new(
        &format!("http://127.0.0.1:{port}"),
        // Generous: Windows takes about two seconds to refuse a connection to a closed port.
        Duration::from_secs(15),
        Duration::ZERO,
    )
    .unwrap();
    let result = client
        .generate(MODEL, &key(), &payload(), &CancelToken::new())
        .await;
    match result {
        Err(AiFailure::Network(message)) => assert!(!message.contains(KEY), "{message}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_oversized_answer_is_refused_instead_of_buffered() {
    let server =
        TestServer::start(|_| Reply::ok("").body(&vec![b'x'; MAX_BODY_BYTES + 1024])).await;
    match call(&server).await {
        Err(AiFailure::Malformed(why)) => assert!(why.contains("too large"), "{why}"),
        other => panic!("{other:?}"),
    }
}

// ---- cancelling ----------------------------------------------------------------------------

#[tokio::test]
async fn cancelling_stops_a_request_that_is_waiting_for_the_provider() {
    let server = TestServer::start(|_| Reply::ok("{}").delay(Duration::from_secs(3))).await;
    let client = client_for(&server);
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let result = client.generate(MODEL, &key(), &payload(), &cancel).await;
    assert_eq!(result, Err(AiFailure::Cancelled));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_token_cancelled_beforehand_sends_nothing() {
    let server = TestServer::start(|_| Reply::ok("{}")).await;
    let cancel = CancelToken::new();
    cancel.cancel();
    let result = client_for(&server)
        .generate(MODEL, &key(), &payload(), &cancel)
        .await;
    assert_eq!(result, Err(AiFailure::Cancelled));
    assert_eq!(server.hits(), 0);
}

#[tokio::test]
async fn cancelling_during_the_wait_before_a_retry_stops_the_retry() {
    let server = TestServer::start(|_| Reply::status(503)).await;
    let client = GeminiClient::new(
        &format!("http://127.0.0.1:{}", server.port()),
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .unwrap();
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let result = client.generate(MODEL, &key(), &payload(), &cancel).await;
    assert_eq!(result, Err(AiFailure::Cancelled));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(server.hits(), 1, "no retry after cancelling");
}
