//! Fixtures shared by the matching tests: listings in a real in-memory database.

use crate::{
    db::{
        Database,
        images::{images_of_listing, record_hashed, register_images},
        listings::{ListingWrite, upsert_listing},
        testkit::{at, normalized},
    },
    models::{
        characteristics::{Extracted, Fact, PropertyType},
        confidence::Confidence,
        evidence::{Evidence, TextSpan},
    },
};

/// An advertisement as the parser reads it: 3 bedrooms, 1 suite, 2 parking spaces, 180 m² built,
/// a 360 m² lot and a stated full price.
pub const FULL_AD: &str = "Casa com 3 quartos, 1 suíte, 2 vagas de garagem, 180 m² de área construída, lote 360 m². Valor total R$ 450 mil";

fn insert(db: &Database, l: &crate::models::listing::NormalizedListing) -> (i64, i64) {
    let outcome = db
        .with_conn(|c| {
            upsert_listing(
                c,
                &ListingWrite {
                    listing: l,
                    source_fields: None,
                    location: None,
                },
            )
        })
        .unwrap();
    (outcome.listing_id, outcome.property_id)
}

/// Adds a listing; returns `(listing id, property id)`.
pub fn add(db: &Database, adapter: &str, n: u32, title: &str, text: &str) -> (i64, i64) {
    let mut l = normalized(adapter, &format!("https://{adapter}.test/{n}"), text, 0);
    l.title = Some(title.to_owned());
    insert(db, &l)
}

/// Like [`add`], with a property type the parser does not yet extract, injected directly.
pub fn add_typed(
    db: &Database,
    adapter: &str,
    n: u32,
    text: &str,
    property_type: PropertyType,
) -> (i64, i64) {
    let mut l = normalized(adapter, &format!("https://{adapter}.test/{n}"), text, 0);
    l.facts.characteristics.property_type = Fact::Known(Extracted {
        value: property_type,
        confidence: Confidence::High,
        evidence: Evidence {
            span: TextSpan { start: 0, end: 4 },
            snippet: "casa".to_owned(),
            field: None,
        },
        warnings: vec![],
    });
    insert(db, &l)
}

/// Sets where a property is, as the region resolver would have.
pub fn place(db: &Database, property: i64, municipality: &str, neighborhood: Option<&str>) {
    db.with_conn(|c| {
        c.execute(
            "UPDATE properties SET municipality = ?1, neighborhood = ?2 WHERE id = ?3",
            rusqlite::params![municipality, neighborhood, property],
        )?;
        Ok(())
    })
    .unwrap();
}

/// Registers an image of `listing` and records it as hashed; returns the image id.
pub fn hash_image(db: &Database, listing: i64, url: &str, hash: u64) -> i64 {
    db.with_conn(|c| register_images(c, listing, &[url.to_owned()], 10, at(0)))
        .unwrap();
    let id = db
        .with_conn(|c| images_of_listing(c, listing))
        .unwrap()
        .into_iter()
        .find(|i| i.source_url.as_deref() == Some(url))
        .unwrap()
        .id;
    db.with_conn(|c| record_hashed(c, id, "images/x/0.png", 10, 10, hash, at(1)))
        .unwrap();
    id
}
