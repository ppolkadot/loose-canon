use std::{io::Cursor, sync::Arc, time::Duration};

use image::{DynamicImage, ImageFormat, imageops};
use tokio::sync::Semaphore;

use super::*;
use crate::{
    db::{
        Database,
        images::{StoredImage, images_of_listing, register_images},
        listings::{ListingWrite, upsert_listing},
        testkit::{at, normalized},
    },
    images::{
        decode::{HashedImage, ImageError, hash_image, tests::png_claiming},
        phash::{hamming_distance, perceptual_hash, tests::scene},
    },
    models::image::ImageProcessingStatus,
    sources::testserver::{Reply, TestServer},
};

fn png(seed: u32) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(scene(seed, 160, 120))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn source(adapter: &str) -> ImageSource {
    let mut config = SourceConfig::new(vec!["127.0.0.1".into()]);
    config.min_request_interval = Duration::ZERO;
    config.request_timeout = Duration::from_secs(2);
    config.connect_timeout = Duration::from_secs(1);
    config.max_retries = 2;
    config.backoff_base = Duration::from_millis(5);
    config.backoff_max = Duration::from_millis(20);
    config.max_image_bytes = 64 * 1024;
    config.max_concurrency = 2;
    ImageSource {
        adapter_id: adapter.to_owned(),
        config,
    }
}

fn deps(db: &Arc<Database>) -> CollectionDeps {
    CollectionDeps {
        db: Arc::clone(db),
        global_limit: Arc::new(Semaphore::new(8)),
        clock: Arc::new(chrono::Utc::now),
    }
}

/// A listing for `adapter` with the given image URLs registered.
fn listing_with_images(db: &Arc<Database>, adapter: &str, n: u32, urls: &[String]) -> i64 {
    let l = normalized(
        adapter,
        &format!("https://{adapter}.test/{n}"),
        "Valor total R$ 350 mil",
        0,
    );
    let listing_id = db
        .with_conn(|c| {
            upsert_listing(
                c,
                &ListingWrite {
                    listing: &l,
                    source_fields: None,
                    location: None,
                },
            )
        })
        .unwrap()
        .listing_id;
    db.with_conn(|c| register_images(c, listing_id, urls, 30, at(0)))
        .unwrap();
    listing_id
}

fn images(db: &Arc<Database>, listing_id: i64) -> Vec<StoredImage> {
    db.with_conn(|c| images_of_listing(c, listing_id)).unwrap()
}

async fn process(
    db: &Arc<Database>,
    sources: &[ImageSource],
    root: &Path,
    batch: u32,
) -> ImageReport {
    process_pending_images(&deps(db), sources, root, batch, &CancelToken::new())
        .await
        .unwrap()
}

fn files_under(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>, root: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out, root);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out, root);
    out.sort();
    out
}

#[tokio::test]
async fn an_image_is_downloaded_hashed_cached_and_recorded() {
    let body = png(5);
    let server = TestServer::start({
        let body = body.clone();
        move |_| {
            Reply::status(200)
                .header("Content-Type", "image/png")
                .body(&body)
        }
    })
    .await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let id = listing_with_images(&db, "alpha", 1, &[server.url("/a.png")]);

    let report = process(&db, &[source("alpha")], dir.path(), 10).await;
    assert_eq!(
        (
            report.attempted,
            report.hashed,
            report.failed,
            report.retry_later
        ),
        (1, 1, 0, 0)
    );

    let row = &images(&db, id)[0];
    assert_eq!(row.status, ImageProcessingStatus::Hashed);
    assert_eq!((row.width, row.height), (Some(160), Some(120)));
    assert_eq!(
        row.phash,
        Some(perceptual_hash(&image::load_from_memory(&body).unwrap()))
    );
    assert_eq!(row.phash_algorithm.as_deref(), Some("dct64-v1"));
    let relative = row.local_path.as_deref().unwrap();
    assert_eq!(relative, format!("images/{id}/0.png"));
    assert_eq!(
        std::fs::read(crate::images::cache::resolve(dir.path(), relative).unwrap()).unwrap(),
        body
    );
    assert_eq!(files_under(dir.path()), vec![relative.to_owned()]);
}

#[tokio::test]
async fn processing_is_idempotent_a_second_call_downloads_nothing() {
    let body = png(6);
    let server = TestServer::start(move |_| Reply::ok("").body(&body)).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    listing_with_images(
        &db,
        "alpha",
        1,
        &[server.url("/a.png"), server.url("/b.png")],
    );
    process(&db, &[source("alpha")], dir.path(), 10).await;
    assert_eq!(server.hits(), 2);
    let again = process(&db, &[source("alpha")], dir.path(), 10).await;
    assert_eq!(again, ImageReport::default());
    assert_eq!(server.hits(), 2);
}

#[tokio::test]
async fn permanent_problems_are_failed_with_a_reason_and_leave_no_file() {
    let good = png(7);
    let big = vec![0xAB_u8; 70 * 1024];
    let truncated = good[..good.len() / 2].to_vec();
    let bomb = png_claiming(50_000, 50_000);
    let server = TestServer::start(move |req| match req.path.as_str() {
        "/ok.png" => Reply::ok("").body(&good),
        "/corrupt.png" => Reply::ok("").body(&truncated),
        "/page.png" => Reply::ok("<html>not an image</html>"),
        "/bomb.png" => Reply::ok("").body(&bomb),
        "/big.png" => Reply::ok("").body(&big),
        "/gone.png" => Reply::status(404),
        "/denied.png" => Reply::status(403),
        _ => Reply::status(500),
    })
    .await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let names = ["ok", "corrupt", "page", "bomb", "big", "gone", "denied"];
    let urls: Vec<String> = names
        .iter()
        .map(|n| server.url(&format!("/{n}.png")))
        .collect();
    let id = listing_with_images(&db, "alpha", 1, &urls);

    let report = process(&db, &[source("alpha")], dir.path(), 20).await;
    assert_eq!(
        (
            report.attempted,
            report.hashed,
            report.failed,
            report.retry_later
        ),
        (7, 1, 6, 0)
    );

    let rows = images(&db, id);
    let outcome = |name: &str| {
        let row = rows
            .iter()
            .find(|r| {
                r.source_url
                    .as_deref()
                    .unwrap()
                    .ends_with(&format!("/{name}.png"))
            })
            .unwrap();
        (row.status, row.error.clone())
    };
    assert_eq!(outcome("ok"), (ImageProcessingStatus::Hashed, None));
    for (name, kind) in [
        ("corrupt", "corrupt"),
        ("page", "unsupported_format"),
        ("bomb", "too_many_pixels"),
        ("big", "too_large"),
        ("gone", "not_found"),
        ("denied", "blocked"),
    ] {
        assert_eq!(
            outcome(name),
            (ImageProcessingStatus::Failed, Some(kind.to_owned())),
            "{name}"
        );
    }
    assert_eq!(
        files_under(dir.path()).len(),
        1,
        "only the good image was cached"
    );
}

#[tokio::test]
async fn transient_failures_stay_pending_and_succeed_on_a_later_run() {
    let body = png(8);
    // The fetcher makes 1 + max_retries = 3 attempts in the first call, all failing.
    let server = TestServer::start(move |req| {
        if req.hit <= 3 {
            Reply::status(503)
        } else {
            Reply::ok("").body(&body)
        }
    })
    .await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let id = listing_with_images(&db, "alpha", 1, &[server.url("/a.png")]);

    let first = process(&db, &[source("alpha")], dir.path(), 10).await;
    assert_eq!(
        (first.attempted, first.hashed, first.retry_later),
        (1, 0, 1)
    );
    let row = &images(&db, id)[0];
    assert_eq!(
        (row.status, row.error.as_deref()),
        (ImageProcessingStatus::Pending, Some("server_error"))
    );

    let second = process(&db, &[source("alpha")], dir.path(), 10).await;
    assert_eq!((second.hashed, second.retry_later), (1, 0));
    let row = &images(&db, id)[0];
    assert_eq!(
        (row.status, row.error.clone()),
        (ImageProcessingStatus::Hashed, None)
    );
}

#[tokio::test]
async fn an_image_host_outside_the_allow_list_is_skipped_without_a_request() {
    let server = TestServer::start(|_| Reply::ok("never asked")).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let foreign = format!("http://localhost:{}/x.png", server.port());
    let id = listing_with_images(&db, "alpha", 1, &[foreign]);
    let report = process(&db, &[source("alpha")], dir.path(), 10).await;
    assert_eq!((report.skipped, report.failed, report.hashed), (1, 0, 0));
    let row = &images(&db, id)[0];
    assert_eq!(
        (row.status, row.error.as_deref()),
        (ImageProcessingStatus::Skipped, Some("disallowed"))
    );
    assert_eq!(server.hits(), 0);
}

#[tokio::test]
async fn a_rate_limited_source_gets_no_further_image_requests_in_the_same_call() {
    let server = TestServer::start(|_| Reply::status(429).header("Retry-After", "30")).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let urls: Vec<String> = (0..5).map(|i| server.url(&format!("/{i}.png"))).collect();
    let id = listing_with_images(&db, "alpha", 1, &urls);
    let mut src = source("alpha");
    src.config.max_concurrency = 1;
    let report = process(&db, &[src], dir.path(), 10).await;
    assert_eq!(report.retry_later, 1);
    assert_eq!(report.not_attempted, 4);
    assert_eq!(server.hits(), 1, "one 429 is enough to stop");
    let rows = images(&db, id);
    assert!(
        rows.iter()
            .all(|r| r.status == ImageProcessingStatus::Pending)
    );
    assert_eq!(rows[0].error.as_deref(), Some("rate_limited"));
}

#[tokio::test]
async fn images_of_unregistered_sources_are_left_untouched() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let id = listing_with_images(&db, "ghost", 1, &["https://img.test/a.png".to_owned()]);
    let report = process(&db, &[source("alpha")], dir.path(), 10).await;
    assert_eq!(report.unavailable_source, 1);
    assert_eq!(report.attempted, 0);
    let row = &images(&db, id)[0];
    assert_eq!(
        (row.status, row.error.clone()),
        (ImageProcessingStatus::Pending, None)
    );
}

#[tokio::test]
async fn cancelling_before_the_job_attempts_nothing_and_leaves_everything_pending() {
    let server = TestServer::start(|_| Reply::ok("x")).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let id = listing_with_images(
        &db,
        "alpha",
        1,
        &[server.url("/a.png"), server.url("/b.png")],
    );
    let cancel = CancelToken::new();
    cancel.cancel();
    let report = process_pending_images(&deps(&db), &[source("alpha")], dir.path(), 10, &cancel)
        .await
        .unwrap();
    assert!(report.cancelled);
    assert_eq!((report.attempted, report.hashed), (0, 0));
    assert_eq!(server.hits(), 0);
    assert!(
        images(&db, id)
            .iter()
            .all(|r| r.status == ImageProcessingStatus::Pending)
    );
}

#[tokio::test]
async fn cancelling_mid_download_returns_promptly_and_leaves_the_image_pending() {
    let server = TestServer::start(|_| Reply::ok("slow").delay(Duration::from_secs(5))).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let id = listing_with_images(&db, "alpha", 1, &[server.url("/a.png")]);
    let cancel = CancelToken::new();
    let task = {
        let (db, root, cancel) = (Arc::clone(&db), dir.path().to_path_buf(), cancel.clone());
        tokio::spawn(async move {
            process_pending_images(&deps(&db), &[source("alpha")], &root, 10, &cancel).await
        })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let started = std::time::Instant::now();
    cancel.cancel();
    let report = task.await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(report.cancelled);
    assert_eq!(images(&db, id)[0].status, ImageProcessingStatus::Pending);
    let leftover = files_under(dir.path());
    assert_eq!(leftover.len(), 0, "{leftover:?}");
}

#[tokio::test]
async fn the_batch_size_bounds_one_call() {
    let body = png(9);
    let server = TestServer::start(move |_| Reply::ok("").body(&body)).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let urls: Vec<String> = (0..5).map(|i| server.url(&format!("/{i}.png"))).collect();
    listing_with_images(&db, "alpha", 1, &urls);
    let report = process(&db, &[source("alpha")], dir.path(), 2).await;
    assert_eq!((report.attempted, report.hashed), (2, 2));
    assert_eq!(server.hits(), 2);
    let rest = process(&db, &[source("alpha")], dir.path(), 10).await;
    assert_eq!(rest.hashed, 3);
}

#[tokio::test]
async fn near_duplicate_downloads_end_up_with_close_hashes() {
    let original = scene(31, 320, 240);
    let mut a = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(original.clone())
        .write_to(&mut a, ImageFormat::Png)
        .unwrap();
    let small = imageops::resize(&original, 200, 150, imageops::FilterType::Triangle);
    let mut b = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(small)
        .write_to(&mut b, ImageFormat::Jpeg)
        .unwrap();
    let (a, b) = (a.into_inner(), b.into_inner());
    let server = TestServer::start(move |req| {
        if req.path == "/a" {
            Reply::ok("").body(&a)
        } else {
            Reply::ok("").body(&b)
        }
    })
    .await;

    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let l1 = listing_with_images(&db, "alpha", 1, &[server.url("/a")]);
    let l2 = listing_with_images(&db, "alpha", 2, &[server.url("/b")]);
    process(&db, &[source("alpha")], dir.path(), 10).await;
    let (h1, h2) = (
        images(&db, l1)[0].phash.unwrap(),
        images(&db, l2)[0].phash.unwrap(),
    );
    assert!(
        hamming_distance(h1, h2) <= 6,
        "{} bits apart",
        hamming_distance(h1, h2)
    );
}

#[tokio::test]
async fn a_refusal_stops_further_requests_to_that_source_in_the_same_call() {
    let server = TestServer::start(|_| Reply::status(403)).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let urls: Vec<String> = (0..5).map(|i| server.url(&format!("/{i}.png"))).collect();
    let id = listing_with_images(&db, "alpha", 1, &urls);
    let mut src = source("alpha");
    src.config.max_concurrency = 1;
    let report = process(&db, &[src], dir.path(), 10).await;
    assert_eq!((report.failed, report.not_attempted), (1, 4));
    assert_eq!(
        server.hits(),
        1,
        "a 403 is neither retried nor probed again"
    );
    let rows = images(&db, id);
    assert_eq!(
        (rows[0].status, rows[0].error.as_deref()),
        (ImageProcessingStatus::Failed, Some("blocked"))
    );
    assert!(
        rows[1..]
            .iter()
            .all(|r| r.status == ImageProcessingStatus::Pending && r.error.is_none())
    );
}

#[tokio::test]
async fn images_of_unregistered_sources_ahead_in_the_queue_do_not_starve_the_rest() {
    let body = png(10);
    let server = TestServer::start(move |_| Reply::ok("").body(&body)).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    listing_with_images(
        &db,
        "ghost",
        1,
        &[
            "https://img.test/1.png".into(),
            "https://img.test/2.png".into(),
            "https://img.test/3.png".into(),
        ],
    );
    let id = listing_with_images(&db, "alpha", 1, &[server.url("/a.png")]);
    let report = process(&db, &[source("alpha")], dir.path(), 1).await;
    assert_eq!((report.hashed, report.unavailable_source), (1, 3));
    assert_eq!(images(&db, id)[0].status, ImageProcessingStatus::Hashed);
}

fn explodes_on_boom(
    bytes: &[u8],
    limits: &ImageLimits,
) -> std::result::Result<HashedImage, ImageError> {
    assert!(bytes != b"boom", "decoder bug");
    hash_image(bytes, limits)
}

#[tokio::test]
async fn a_decoder_that_panics_on_hostile_bytes_fails_that_image_and_nothing_else() {
    let good = png(11);
    let server = TestServer::start(move |req| {
        if req.path == "/bad.png" {
            Reply::ok("boom")
        } else {
            Reply::ok("").body(&good)
        }
    })
    .await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let id = listing_with_images(
        &db,
        "alpha",
        1,
        &[server.url("/bad.png"), server.url("/ok.png")],
    );
    let report = run(
        &deps(&db),
        &[source("alpha")],
        dir.path(),
        10,
        &CancelToken::new(),
        explodes_on_boom,
    )
    .await
    .unwrap();
    assert_eq!(
        (report.hashed, report.failed, report.internal_errors),
        (1, 1, 0)
    );
    let rows = images(&db, id);
    assert_eq!(
        (rows[0].status, rows[0].error.as_deref()),
        (ImageProcessingStatus::Failed, Some("decoder_crashed"))
    );
    assert_eq!(rows[1].status, ImageProcessingStatus::Hashed);
    assert_eq!(files_under(dir.path()).len(), 1);
}

#[tokio::test]
async fn a_cache_that_cannot_be_written_leaves_the_image_pending_for_a_later_run() {
    let body = png(12);
    let server = TestServer::start(move |_| Reply::ok("").body(&body)).await;
    let db = Arc::new(Database::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    // A file where the cache directory must go: creating `images/` under it fails.
    let blocked_root = dir.path().join("not-a-directory");
    std::fs::write(&blocked_root, b"x").unwrap();
    let id = listing_with_images(&db, "alpha", 1, &[server.url("/a.png")]);
    let report = process(&db, &[source("alpha")], &blocked_root, 10).await;
    assert_eq!((report.hashed, report.retry_later), (0, 1));
    let row = &images(&db, id)[0];
    assert_eq!(
        (row.status, row.error.as_deref()),
        (ImageProcessingStatus::Pending, Some("cache_write_failed"))
    );
    assert_eq!(
        row.phash, None,
        "no hash is recorded for a file that was not kept"
    );
}
