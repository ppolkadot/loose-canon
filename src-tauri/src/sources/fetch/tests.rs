use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::Semaphore;

use super::*;
use crate::{
    error::Error,
    sources::testserver::{Reply, TestServer},
};

fn cfg() -> SourceConfig {
    let mut c = SourceConfig::new(vec!["127.0.0.1".into()]);
    c.min_request_interval = Duration::ZERO;
    c.request_timeout = Duration::from_secs(2);
    c.connect_timeout = Duration::from_secs(1);
    c.max_retries = 2;
    c.backoff_base = Duration::from_millis(5);
    c.backoff_max = Duration::from_millis(20);
    c.cache_ttl = Duration::ZERO;
    c.max_response_bytes = 4096;
    c.max_concurrency = 4;
    c
}

fn fetcher(config: SourceConfig) -> Fetcher {
    Fetcher::new(config, Arc::new(Semaphore::new(16))).expect("valid config")
}

fn never() -> CancelToken {
    CancelToken::new()
}

#[tokio::test]
async fn fetches_a_page_and_reports_what_it_got() {
    let server = TestServer::start(|_| {
        Reply::ok("hello").header("Content-Type", "text/html; charset=utf-8")
    })
    .await;
    let f = fetcher(cfg());
    let got = f.get(&server.url("/imovel/1"), &never()).await.unwrap();
    assert_eq!(got.status, 200);
    assert_eq!(got.text_lossy(), "hello");
    assert_eq!(
        got.content_type.as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(got.final_url, server.url("/imovel/1"));
    assert_eq!(got.requested_url, server.url("/imovel/1"));
    assert!(!got.from_cache);
}

#[tokio::test]
async fn sends_the_configured_user_agent_and_nothing_pretending_to_be_a_browser() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let server = {
        let seen = Arc::clone(&seen);
        TestServer::start(move |req| {
            *seen.lock().unwrap() = req.header("user-agent").map(str::to_owned);
            Reply::ok("x")
        })
        .await
    };
    let mut c = cfg();
    c.user_agent = "LooseCanon/test (unit)".into();
    fetcher(c).get(&server.url("/"), &never()).await.unwrap();
    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some("LooseCanon/test (unit)")
    );
}

#[tokio::test]
async fn a_response_over_the_size_limit_is_an_error_never_a_silent_truncation() {
    let server = TestServer::start(|req| {
        let size = if req.path == "/exact" { 4096 } else { 5000 };
        Reply::ok(&"a".repeat(size))
    })
    .await;
    let f = fetcher(cfg());
    assert_eq!(
        f.get(&server.url("/exact"), &never())
            .await
            .unwrap()
            .body
            .len(),
        4096
    );
    assert_eq!(
        f.get(&server.url("/big"), &never()).await.unwrap_err(),
        SourceError::TooLarge { limit: 4096 }
    );
}

#[tokio::test]
async fn refusals_gone_pages_and_rate_limits_are_classified_and_never_retried() {
    let server = TestServer::start(|req| match req.path.as_str() {
        "/404" => Reply::status(404),
        "/410" => Reply::status(410),
        "/401" => Reply::status(401),
        "/403" => Reply::status(403),
        "/451" => Reply::status(451),
        "/429" => Reply::status(429).header("Retry-After", "7"),
        "/429-bare" => Reply::status(429),
        "/418" => Reply::status(418),
        _ => Reply::ok("?"),
    })
    .await;
    let f = fetcher(cfg());
    let get = |p: &'static str| {
        let url = server.url(p);
        let f = &f;
        async move { f.get(&url, &CancelToken::new()).await.unwrap_err() }
    };
    assert_eq!(get("/404").await, SourceError::NotFound { status: 404 });
    assert_eq!(get("/410").await, SourceError::NotFound { status: 410 });
    assert_eq!(get("/401").await, SourceError::Blocked { status: 401 });
    assert_eq!(get("/403").await, SourceError::Blocked { status: 403 });
    assert_eq!(get("/451").await, SourceError::Blocked { status: 451 });
    assert_eq!(
        get("/429").await,
        SourceError::RateLimited {
            retry_after_secs: Some(7)
        }
    );
    assert_eq!(
        get("/429-bare").await,
        SourceError::RateLimited {
            retry_after_secs: None
        }
    );
    assert_eq!(
        get("/418").await,
        SourceError::UnexpectedStatus { status: 418 }
    );
    assert_eq!(server.hits(), 8, "none of these may be retried");
}

#[tokio::test]
async fn transient_server_errors_are_retried_a_bounded_number_of_times() {
    let always = TestServer::start(|_| Reply::status(503)).await;
    let f = fetcher(cfg()); // max_retries = 2
    assert_eq!(
        f.get(&always.url("/"), &never()).await.unwrap_err(),
        SourceError::Server { status: 503 }
    );
    assert_eq!(always.hits(), 3, "one attempt plus exactly max_retries");

    let flaky = TestServer::start(|req| {
        if req.hit == 1 {
            Reply::status(500)
        } else {
            Reply::ok("fine")
        }
    })
    .await;
    let got = f.get(&flaky.url("/"), &never()).await.unwrap();
    assert_eq!(got.text_lossy(), "fine");
    assert_eq!(flaky.hits(), 2);
}

#[tokio::test]
async fn slow_servers_time_out_and_the_retries_stay_within_the_bound() {
    let server = TestServer::start(|_| Reply::ok("late").delay(Duration::from_millis(600))).await;
    let mut c = cfg();
    c.request_timeout = Duration::from_millis(120);
    c.max_retries = 1;
    let started = Instant::now();
    let err = fetcher(c)
        .get(&server.url("/"), &never())
        .await
        .unwrap_err();
    assert_eq!(err, SourceError::Timeout);
    assert_eq!(server.hits(), 2);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn redirects_inside_the_allowed_hosts_are_followed() {
    let server = TestServer::start(|req| match req.path.as_str() {
        "/a" => Reply::redirect("/b"),
        _ => Reply::ok("landed"),
    })
    .await;
    let got = fetcher(cfg())
        .get(&server.url("/a"), &never())
        .await
        .unwrap();
    assert_eq!(got.text_lossy(), "landed");
    assert_eq!(got.final_url, server.url("/b"));
    assert_eq!(got.requested_url, server.url("/a"));
    assert_eq!(server.paths(), vec!["/a", "/b"]);
}

#[tokio::test]
async fn a_redirect_to_another_host_is_refused_before_connecting() {
    let server = TestServer::start(|req| {
        Reply::redirect(&format!(
            "http://localhost:{}/elsewhere",
            req.header("host").unwrap().rsplit(':').next().unwrap()
        ))
    })
    .await;
    let err = fetcher(cfg())
        .get(&server.url("/a"), &never())
        .await
        .unwrap_err();
    assert!(matches!(err, SourceError::Disallowed(_)), "{err}");
    assert_eq!(server.hits(), 1, "the foreign host must never be contacted");
}

#[tokio::test]
async fn redirect_loops_stop_at_the_limit() {
    let server = TestServer::start(|_| Reply::redirect("/loop")).await;
    let mut c = cfg();
    c.max_redirects = 3;
    let err = fetcher(c)
        .get(&server.url("/loop"), &never())
        .await
        .unwrap_err();
    assert_eq!(err, SourceError::TooManyRedirects);
    assert!(server.hits() <= 5, "bounded, got {}", server.hits());
}

#[tokio::test]
async fn urls_outside_the_allow_list_or_of_the_wrong_kind_never_leave_the_machine() {
    let server = TestServer::start(|_| Reply::ok("x")).await;
    let f = fetcher(cfg());
    for url in [
        "http://example.org/x",
        "https://127.0.0.1.evil.example/x",
        "ftp://127.0.0.1/x",
        "file:///etc/passwd",
        "javascript:alert(1)",
        &format!("http://user:pw@127.0.0.1:{}/x", server.port()),
        "/relative",
        "",
    ] {
        let err = f.get(url, &never()).await.unwrap_err();
        assert!(
            matches!(err, SourceError::Disallowed(_)),
            "{url:?} -> {err}"
        );
    }
    assert_eq!(server.hits(), 0);
}

#[tokio::test]
async fn per_source_concurrency_is_bounded() {
    let server = TestServer::start(|_| Reply::ok("x").delay(Duration::from_millis(80))).await;
    let mut c = cfg();
    c.max_concurrency = 2;
    let f = Arc::new(fetcher(c));
    let tasks: Vec<_> = (0..6)
        .map(|i| {
            let (f, url) = (Arc::clone(&f), server.url(&format!("/{i}")));
            tokio::spawn(async move { f.get(&url, &CancelToken::new()).await })
        })
        .collect();
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    assert_eq!(server.hits(), 6);
    assert_eq!(
        server.max_concurrent(),
        2,
        "never above the limit, and the limit is actually used"
    );
}

#[tokio::test]
async fn the_global_limit_bounds_all_sources_together() {
    let server = TestServer::start(|_| Reply::ok("x").delay(Duration::from_millis(60))).await;
    let global = Arc::new(Semaphore::new(1));
    let a = Arc::new(Fetcher::new(cfg(), Arc::clone(&global)).unwrap());
    let b = Arc::new(Fetcher::new(cfg(), global).unwrap());
    let mut tasks = Vec::new();
    for i in 0..4 {
        for f in [&a, &b] {
            let (f, url) = (Arc::clone(f), server.url(&format!("/{i}")));
            tasks.push(tokio::spawn(async move {
                f.get(&url, &CancelToken::new()).await
            }));
        }
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    assert_eq!(server.max_concurrent(), 1);
}

#[tokio::test]
async fn request_starts_respect_the_minimum_interval() {
    let server = TestServer::start(|_| Reply::ok("x")).await;
    let mut c = cfg();
    c.min_request_interval = Duration::from_millis(100);
    let f = Arc::new(fetcher(c));
    let tasks: Vec<_> = (0..4)
        .map(|i| {
            let (f, url) = (Arc::clone(&f), server.url(&format!("/{i}")));
            tokio::spawn(async move { f.get(&url, &CancelToken::new()).await })
        })
        .collect();
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    let mut starts = server.start_times();
    starts.sort();
    for pair in starts.windows(2) {
        assert!(
            pair[1] - pair[0] >= Duration::from_millis(70),
            "requests started only {:?} apart",
            pair[1] - pair[0]
        );
    }
}

#[tokio::test]
async fn successful_responses_are_cached_for_the_ttl_only() {
    let server = TestServer::start(|_| Reply::ok("page")).await;
    let mut c = cfg();
    c.cache_ttl = Duration::from_millis(250);
    let f = fetcher(c);
    let first = f.get(&server.url("/p"), &never()).await.unwrap();
    let second = f.get(&server.url("/p"), &never()).await.unwrap();
    assert!(!first.from_cache && second.from_cache);
    assert_eq!(second.body, first.body);
    assert_eq!(server.hits(), 1);
    tokio::time::sleep(Duration::from_millis(320)).await;
    let third = f.get(&server.url("/p"), &never()).await.unwrap();
    assert!(!third.from_cache);
    assert_eq!(server.hits(), 2);
}

#[tokio::test]
async fn failures_are_never_cached() {
    let server = TestServer::start(|req| {
        if req.hit <= 3 {
            Reply::status(500)
        } else {
            Reply::ok("recovered")
        }
    })
    .await;
    let mut c = cfg();
    c.cache_ttl = Duration::from_secs(60);
    let f = fetcher(c);
    assert!(f.get(&server.url("/p"), &never()).await.is_err()); // 3 attempts, all 500
    let got = f.get(&server.url("/p"), &never()).await.unwrap();
    assert_eq!(got.text_lossy(), "recovered");
    assert!(!got.from_cache);
}

#[tokio::test]
async fn the_cache_is_bounded_by_entries_and_by_bytes() {
    let server = TestServer::start(|req| {
        Reply::ok(&"z".repeat(if req.path.starts_with("/big") {
            1000
        } else {
            10
        }))
    })
    .await;

    let mut by_entries = cfg();
    by_entries.cache_ttl = Duration::from_secs(60);
    by_entries.cache_max_entries = 2;
    let f = fetcher(by_entries);
    for p in ["/a", "/b", "/c"] {
        f.get(&server.url(p), &never()).await.unwrap();
    }
    assert!(f.get(&server.url("/c"), &never()).await.unwrap().from_cache);
    assert!(
        !f.get(&server.url("/a"), &never()).await.unwrap().from_cache,
        "oldest entry was evicted"
    );

    let mut by_bytes = cfg();
    by_bytes.cache_ttl = Duration::from_secs(60);
    by_bytes.cache_max_bytes = 1500;
    let f = fetcher(by_bytes);
    f.get(&server.url("/big1"), &never()).await.unwrap();
    f.get(&server.url("/big2"), &never()).await.unwrap();
    assert!(
        f.get(&server.url("/big2"), &never())
            .await
            .unwrap()
            .from_cache
    );
    assert!(
        !f.get(&server.url("/big1"), &never())
            .await
            .unwrap()
            .from_cache,
        "byte budget evicted the oldest"
    );
}

#[tokio::test]
async fn cancelling_before_the_request_sends_nothing() {
    let server = TestServer::start(|_| Reply::ok("x")).await;
    let token = CancelToken::new();
    token.cancel();
    assert_eq!(
        fetcher(cfg())
            .get(&server.url("/"), &token)
            .await
            .unwrap_err(),
        SourceError::Cancelled
    );
    assert_eq!(server.hits(), 0);
}

#[tokio::test]
async fn cancelling_an_in_flight_request_returns_promptly() {
    let server = TestServer::start(|_| Reply::ok("x").delay(Duration::from_secs(5))).await;
    let f = Arc::new(fetcher(cfg()));
    let token = CancelToken::new();
    let task = {
        let (f, url, token) = (Arc::clone(&f), server.url("/"), token.clone());
        tokio::spawn(async move { f.get(&url, &token).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    token.cancel();
    let result = task.await.unwrap();
    assert_eq!(result.unwrap_err(), SourceError::Cancelled);
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[tokio::test]
async fn cancelling_during_a_retry_backoff_returns_promptly() {
    let server = TestServer::start(|_| Reply::status(503)).await;
    let mut c = cfg();
    c.backoff_base = Duration::from_secs(3);
    c.backoff_max = Duration::from_secs(3);
    let f = Arc::new(fetcher(c));
    let token = CancelToken::new();
    let task = {
        let (f, url, token) = (Arc::clone(&f), server.url("/"), token.clone());
        tokio::spawn(async move { f.get(&url, &token).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    token.cancel();
    assert_eq!(task.await.unwrap().unwrap_err(), SourceError::Cancelled);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(server.hits(), 1, "no second attempt after cancelling");
}

#[tokio::test]
async fn cancelling_while_waiting_for_a_rate_limit_slot_returns_promptly() {
    let server = TestServer::start(|_| Reply::ok("x")).await;
    let mut c = cfg();
    c.min_request_interval = Duration::from_secs(3);
    let f = Arc::new(fetcher(c));
    f.get(&server.url("/1"), &never()).await.unwrap();
    let token = CancelToken::new();
    let task = {
        let (f, url, token) = (Arc::clone(&f), server.url("/2"), token.clone());
        tokio::spawn(async move { f.get(&url, &token).await })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let started = Instant::now();
    token.cancel();
    assert_eq!(task.await.unwrap().unwrap_err(), SourceError::Cancelled);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(server.hits(), 1);
}

#[tokio::test]
async fn an_invalid_configuration_is_refused_up_front() {
    let mut c = cfg();
    c.max_retries = 50;
    assert!(matches!(
        Fetcher::new(c, Arc::new(Semaphore::new(1))),
        Err(Error::InvalidConfig { .. })
    ));
}

#[tokio::test]
async fn a_connection_that_cannot_be_made_is_a_network_error() {
    // Bind then drop a listener to get a port nothing listens on.
    let port = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let mut c = cfg();
    c.max_retries = 0;
    c.connect_timeout = Duration::from_millis(300);
    let err = fetcher(c)
        .get(&format!("http://127.0.0.1:{port}/"), &never())
        .await
        .unwrap_err();
    // Linux refuses at once (Network); Windows lets the connect attempt run into the timeout.
    // Either way it is a transient, retryable failure that never yields a body.
    assert!(
        matches!(err, SourceError::Network(_) | SourceError::Timeout),
        "{err}"
    );
    assert!(err.is_retryable());
}

#[tokio::test]
async fn images_have_their_own_larger_limit_and_are_never_cached_in_memory() {
    let server = TestServer::start(|req| {
        Reply::ok(&"i".repeat(if req.path == "/huge" { 9000 } else { 6000 }))
    })
    .await;
    let mut c = cfg();
    c.max_response_bytes = 4096;
    c.max_image_bytes = 8192;
    c.cache_ttl = Duration::from_secs(60);
    let f = fetcher(c);

    assert_eq!(
        f.get(&server.url("/p.jpg"), &never()).await.unwrap_err(),
        SourceError::TooLarge { limit: 4096 }
    );
    let first = f.get_image(&server.url("/p.jpg"), &never()).await.unwrap();
    assert_eq!(first.body.len(), 6000);
    let second = f.get_image(&server.url("/p.jpg"), &never()).await.unwrap();
    assert!(!first.from_cache && !second.from_cache);
    assert_eq!(
        server.hits(),
        3,
        "the page attempt plus two image downloads: images are not cached"
    );
    assert_eq!(
        f.get_image(&server.url("/huge"), &never())
            .await
            .unwrap_err(),
        SourceError::TooLarge { limit: 8192 }
    );
}
