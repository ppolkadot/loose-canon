use super::*;
use crate::{
    db::{
        Database,
        listings::{ListingWrite, upsert_listing},
        testkit::{at, mem, normalized},
    },
    error::Error,
    images::phash::ALGORITHM,
};

fn listing(db: &Database, adapter: &str, n: u32) -> i64 {
    let l = normalized(
        adapter,
        &format!("https://{adapter}.test/{n}"),
        "Valor total R$ 350 mil",
        0,
    );
    db.with_conn(|c| {
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
    .listing_id
}

fn urls(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!("https://img.test/{i}.jpg"))
        .collect()
}

fn register(db: &Database, listing_id: i64, urls: &[String], max: usize) -> usize {
    db.with_conn(|c| register_images(c, listing_id, urls, max, at(0)))
        .unwrap()
}

fn images(db: &Database, listing_id: i64) -> Vec<StoredImage> {
    db.with_conn(|c| images_of_listing(c, listing_id)).unwrap()
}

#[test]
fn registered_images_start_pending_in_order_with_their_source_urls() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    assert_eq!(register(&db, l, &urls(3), 10), 3);
    let rows = images(&db, l);
    assert_eq!(rows.len(), 3);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.position, u32::try_from(i).unwrap());
        assert_eq!(
            row.source_url.as_deref(),
            Some(format!("https://img.test/{i}.jpg").as_str())
        );
        assert_eq!(row.status, ImageProcessingStatus::Pending);
        assert_eq!(
            (row.phash, row.local_path.clone(), row.error.clone()),
            (None, None, None)
        );
    }
}

#[test]
fn registration_is_idempotent_and_positions_continue() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    assert_eq!(register(&db, l, &urls(2), 10), 2);
    assert_eq!(
        register(&db, l, &urls(2), 10),
        0,
        "same URLs again add nothing"
    );
    let mut more = urls(3);
    more.push("https://img.test/extra.jpg".into());
    assert_eq!(register(&db, l, &more, 10), 2, "only the two new URLs");
    let positions: Vec<u32> = images(&db, l).iter().map(|i| i.position).collect();
    assert_eq!(positions, vec![0, 1, 2, 3]);
}

#[test]
fn a_listing_never_holds_more_images_than_the_cap() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    assert_eq!(register(&db, l, &urls(10), 3), 3);
    assert_eq!(register(&db, l, &urls(10), 3), 0, "already at the cap");
    assert_eq!(images(&db, l).len(), 3);
    assert_eq!(
        register(&db, l, &urls(10), 5),
        2,
        "a raised cap admits more, counting what exists"
    );
}

#[test]
fn unusable_urls_are_ignored() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    let list = vec![
        "https://img.test/ok.jpg".to_owned(),
        "javascript:alert(1)".into(),
        "data:image/png;base64,AAAA".into(),
        "ftp://img.test/x.jpg".into(),
        "https://user:pw@img.test/x.jpg".into(),
        "/relative.jpg".into(),
        String::new(),
        "https://img.test/ok.jpg".into(),
    ];
    assert_eq!(register(&db, l, &list, 10), 1);
}

#[test]
fn only_pending_images_are_queued_oldest_first_with_their_adapter() {
    let db = mem();
    let a = listing(&db, "alpha", 1);
    let b = listing(&db, "beta", 1);
    register(&db, a, &urls(2), 10);
    register(&db, b, &urls(1), 10);
    let queue = db
        .with_conn(|c| pending_images(c, &["alpha", "beta"], 10))
        .unwrap();
    assert_eq!(queue.len(), 3);
    assert_eq!(
        queue
            .iter()
            .map(|p| p.adapter_id.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "alpha", "beta"]
    );
    assert_eq!(
        db.with_conn(|c| pending_images(c, &["alpha", "beta"], 2))
            .unwrap()
            .len(),
        2,
        "limit applies"
    );

    db.with_conn(|c| {
        record_final(
            c,
            queue[0].id,
            ImageProcessingStatus::Failed,
            "corrupt",
            at(1),
        )
    })
    .unwrap();
    db.with_conn(|c| record_hashed(c, queue[1].id, "images/1/1.png", 10, 10, 5, at(1)))
        .unwrap();
    let rest = db
        .with_conn(|c| pending_images(c, &["alpha", "beta"], 10))
        .unwrap();
    assert_eq!(rest.len(), 1, "finished images leave the queue");
    assert_eq!(rest[0].id, queue[2].id);
}

#[test]
fn the_queue_covers_only_the_given_adapters_so_others_cannot_starve_it() {
    let db = mem();
    // Images of an adapter nobody is running come first (lower ids) and are plentiful.
    let ghost = listing(&db, "ghost", 1);
    register(&db, ghost, &urls(3), 10);
    let alpha = listing(&db, "alpha", 1);
    register(&db, alpha, &urls(2), 10);

    let queue = db.with_conn(|c| pending_images(c, &["alpha"], 1)).unwrap();
    assert_eq!(
        queue.len(),
        1,
        "the limit counts only images that can be processed"
    );
    assert_eq!(
        (queue[0].adapter_id.as_str(), queue[0].listing_id),
        ("alpha", alpha)
    );

    assert_eq!(
        db.with_conn(|c| pending_images(c, &["alpha"], 10))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        db.with_conn(|c| pending_images(c, &["ghost", "alpha"], 10))
            .unwrap()
            .len(),
        5
    );
    assert_eq!(
        db.with_conn(|c| count_pending_outside(c, &["alpha"]))
            .unwrap(),
        3
    );
    assert_eq!(
        db.with_conn(|c| count_pending_outside(c, &["ghost", "alpha"]))
            .unwrap(),
        0
    );
}

#[test]
fn no_adapters_means_nothing_to_process_and_everything_outside() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    register(&db, l, &urls(2), 10);
    let queue = db.with_conn(|c| pending_images(c, &[], 10)).unwrap();
    assert_eq!(queue.len(), 0, "{queue:?}");
    assert_eq!(db.with_conn(|c| count_pending_outside(c, &[])).unwrap(), 2);
}

#[test]
fn adapter_ids_are_bound_as_values_never_spliced_into_the_query() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    register(&db, l, &urls(1), 10);
    let hostile = "x') OR 1=1 --";
    let queue = db.with_conn(|c| pending_images(c, &[hostile], 10)).unwrap();
    assert_eq!(queue.len(), 0, "{queue:?}");
    assert_eq!(
        db.with_conn(|c| count_pending_outside(c, &[hostile]))
            .unwrap(),
        1
    );
}

#[test]
fn only_finished_images_are_not_counted_as_pending_elsewhere() {
    let db = mem();
    let l = listing(&db, "ghost", 1);
    register(&db, l, &urls(2), 10);
    let rows = images(&db, l);
    db.with_conn(|c| {
        record_final(
            c,
            rows[0].id,
            ImageProcessingStatus::Failed,
            "corrupt",
            at(1),
        )
    })
    .unwrap();
    assert_eq!(
        db.with_conn(|c| count_pending_outside(c, &["alpha"]))
            .unwrap(),
        1
    );
}

#[test]
fn a_hashed_image_keeps_its_metadata_and_the_full_64_bit_hash() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    register(&db, l, &urls(1), 10);
    let id = images(&db, l)[0].id;
    let hash = 0xF0F0_F0F0_0F0F_0F0F_u64; // top bit set
    assert!(
        db.with_conn(|c| record_hashed(c, id, "images/7/0.jpg", 640, 480, hash, at(5)))
            .unwrap()
    );
    let row = &images(&db, l)[0];
    assert_eq!(row.status, ImageProcessingStatus::Hashed);
    assert_eq!((row.width, row.height), (Some(640), Some(480)));
    assert_eq!(row.phash, Some(hash));
    assert_eq!(row.phash_algorithm.as_deref(), Some(ALGORITHM));
    assert_eq!(row.local_path.as_deref(), Some("images/7/0.jpg"));
    assert_eq!(row.error, None);
}

#[test]
fn hashing_an_unknown_image_changes_nothing() {
    let db = mem();
    assert!(
        !db.with_conn(|c| record_hashed(c, 999, "images/1/0.jpg", 1, 1, 1, at(0)))
            .unwrap()
    );
    assert!(
        !db.with_conn(|c| record_final(c, 999, ImageProcessingStatus::Failed, "x", at(0)))
            .unwrap()
    );
    assert!(
        !db.with_conn(|c| record_transient(c, 999, "timeout"))
            .unwrap()
    );
}

#[test]
fn absolute_or_drive_qualified_cache_paths_are_refused_by_the_schema() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    register(&db, l, &urls(1), 10);
    let id = images(&db, l)[0].id;
    for bad in [
        "/etc/passwd",
        "C:\\Users\\x.jpg",
        "\\\\server\\share\\x.jpg",
    ] {
        let err = db
            .with_conn(|c| record_hashed(c, id, bad, 1, 1, 1, at(0)))
            .unwrap_err();
        assert!(matches!(err, Error::Database(_)), "{bad}: {err}");
    }
}

#[test]
fn permanent_outcomes_are_final_and_record_the_reason() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    register(&db, l, &urls(2), 10);
    let rows = images(&db, l);
    db.with_conn(|c| {
        record_final(
            c,
            rows[0].id,
            ImageProcessingStatus::Failed,
            "corrupt",
            at(1),
        )
    })
    .unwrap();
    db.with_conn(|c| {
        record_final(
            c,
            rows[1].id,
            ImageProcessingStatus::Skipped,
            "disallowed",
            at(1),
        )
    })
    .unwrap();
    let rows = images(&db, l);
    assert_eq!(
        (rows[0].status, rows[0].error.as_deref()),
        (ImageProcessingStatus::Failed, Some("corrupt"))
    );
    assert_eq!(
        (rows[1].status, rows[1].error.as_deref()),
        (ImageProcessingStatus::Skipped, Some("disallowed"))
    );
    let queue = db
        .with_conn(|c| pending_images(c, &["alpha", "beta"], 10))
        .unwrap();
    assert_eq!(queue.len(), 0, "{queue:?}");
}

#[test]
fn only_failed_and_skipped_may_be_recorded_as_final() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    register(&db, l, &urls(1), 10);
    let id = images(&db, l)[0].id;
    for status in [
        ImageProcessingStatus::Pending,
        ImageProcessingStatus::Hashed,
        ImageProcessingStatus::Embedded,
    ] {
        let err = db
            .with_conn(|c| record_final(c, id, status, "x", at(0)))
            .unwrap_err();
        assert!(matches!(err, Error::Model(_)), "{status}: {err}");
    }
}

#[test]
fn a_transient_failure_keeps_the_image_in_the_queue_with_the_reason_visible() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    register(&db, l, &urls(1), 10);
    let id = images(&db, l)[0].id;
    assert!(
        db.with_conn(|c| record_transient(c, id, "timeout"))
            .unwrap()
    );
    let row = &images(&db, l)[0];
    assert_eq!(
        (row.status, row.error.as_deref()),
        (ImageProcessingStatus::Pending, Some("timeout"))
    );
    assert_eq!(
        db.with_conn(|c| pending_images(c, &["alpha", "beta"], 10))
            .unwrap()
            .len(),
        1
    );
    // Success clears the error.
    db.with_conn(|c| record_hashed(c, id, "images/1/0.jpg", 2, 2, 9, at(2)))
        .unwrap();
    assert_eq!(images(&db, l)[0].error, None);
}

#[test]
fn hashes_lists_only_images_hashed_with_the_current_algorithm() {
    let db = mem();
    let a = listing(&db, "alpha", 1);
    let b = listing(&db, "beta", 1);
    register(&db, a, &urls(2), 10);
    register(&db, b, &urls(1), 10);
    let ids: Vec<i64> = images(&db, a)
        .iter()
        .chain(images(&db, b).iter())
        .map(|i| i.id)
        .collect();
    db.with_conn(|c| record_hashed(c, ids[0], "images/1/0.jpg", 2, 2, 111, at(0)))
        .unwrap();
    db.with_conn(|c| record_hashed(c, ids[2], "images/2/0.jpg", 2, 2, u64::MAX, at(0)))
        .unwrap();
    // A hash produced by some other algorithm must never be compared with ours.
    db.with_conn(|c| {
        c.execute(
            "UPDATE property_images SET phash = 7, phash_algorithm = 'other-v9', processing_status = 'hashed' WHERE id = ?1",
            [ids[1]],
        )?;
        Ok(())
    })
    .unwrap();
    let mut got = db.with_conn(|c| hashes(c)).unwrap();
    got.sort_unstable();
    assert_eq!(got, vec![(ids[0], a, 111), (ids[2], b, u64::MAX)]);
}

#[test]
fn every_processing_status_is_accepted_by_the_database() {
    let db = mem();
    let l = listing(&db, "alpha", 1);
    register(&db, l, &urls(1), 10);
    for status in ImageProcessingStatus::ALL {
        db.with_conn(|c| {
            c.execute(
                "UPDATE property_images SET processing_status = ?1 WHERE listing_id = ?2",
                rusqlite::params![status.as_str(), l],
            )?;
            Ok(())
        })
        .unwrap();
    }
}
