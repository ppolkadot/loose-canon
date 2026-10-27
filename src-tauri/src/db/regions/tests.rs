use super::*;
use crate::{db::Database, error::Error};

fn mem() -> Database {
    Database::open_in_memory().unwrap()
}

fn seeded() -> Database {
    let db = mem();
    db.with_conn(|c| seed_regions(c, &parse_seed(DEFAULT_SEED_JSON)?))
        .unwrap();
    db
}

fn resolve(db: &Database, text: &str) -> Option<ResolvedLocation> {
    db.with_conn(|c| Ok(LocationIndex::load(c)?.resolve(text)))
        .unwrap()
}

fn place(r: Option<&ResolvedLocation>) -> (Option<&str>, Option<&str>) {
    match r {
        Some(r) => (r.municipality.as_deref(), r.neighborhood.as_deref()),
        None => (None, None),
    }
}

fn seed_json(regions: &str) -> String {
    format!(r#"{{ "version": 1, "regions": [{regions}] }}"#)
}

// ---- the bundled seed --------------------------------------------------------------------

#[test]
fn bundled_seed_is_valid_and_covers_the_regions_in_the_specification() {
    let seed = parse_seed(DEFAULT_SEED_JSON).unwrap();
    let names: Vec<&str> = seed.regions.iter().map(|r| r.name.as_str()).collect();
    for expected in [
        "Goiânia",
        "Setor Bueno",
        "Setor Marista",
        "Jardim Goiás",
        "Parque Amazônia",
        "Aparecida de Goiânia",
        "Vila Brasília",
        "Garavelo",
        "Cidade Livre",
        "Buriti Sereno",
        "Hidrolândia",
    ] {
        assert!(names.contains(&expected), "seed lacks {expected}");
    }
    assert!(
        seed.regions
            .iter()
            .any(|r| r.region_type == RegionType::Corridor)
    );
}

#[test]
fn seeding_creates_the_whole_hierarchy() {
    let db = seeded();
    db.with_conn(|c| {
        let rows = list_regions(c, false)?;
        let seed = parse_seed(DEFAULT_SEED_JSON)?;
        assert_eq!(rows.len(), seed.regions.len());
        let bueno = rows
            .iter()
            .find(|r| r.slug == "br-go-goiania-setor-bueno")
            .unwrap();
        let goiania = rows.iter().find(|r| r.slug == "br-go-goiania").unwrap();
        assert_eq!(bueno.parent_id, Some(goiania.id));
        assert_eq!(bueno.municipality.as_deref(), Some("Goiânia"));
        assert!(bueno.enabled);
        assert_eq!(bueno.aliases, vec!["St. Bueno".to_owned()]);
        let state = rows.iter().find(|r| r.slug == "br-go").unwrap();
        assert_eq!(
            (state.parent_id, state.municipality.as_deref()),
            (None, None)
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn seeding_twice_changes_nothing() {
    let db = mem();
    let seed = parse_seed(DEFAULT_SEED_JSON).unwrap();
    let first = db.with_conn(|c| seed_regions(c, &seed)).unwrap();
    assert_eq!(first.inserted, seed.regions.len());
    let second = db.with_conn(|c| seed_regions(c, &seed)).unwrap();
    assert_eq!(
        second,
        SeedReport {
            inserted: 0,
            updated: 0,
            unchanged: seed.regions.len()
        }
    );
}

#[test]
fn reseeding_updates_definitions_but_keeps_user_choices_and_extra_regions() {
    let db = seeded();
    db.with_conn(|c| {
        c.execute(
            "UPDATE regions SET enabled = 0 WHERE slug = 'br-go-goiania-setor-bueno'",
            [],
        )?;
        c.execute(
            "INSERT INTO regions (slug, state_code, municipality, name, region_type)
             VALUES ('custom-user-region', 'GO', 'Goiânia', 'Meu Bairro', 'neighborhood')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let mut seed = parse_seed(DEFAULT_SEED_JSON).unwrap();
    seed.regions
        .iter_mut()
        .find(|r| r.slug == "br-go-goiania-setor-marista")
        .unwrap()
        .name = "Setor Marista (renomeado)".into();
    let report = db.with_conn(|c| seed_regions(c, &seed)).unwrap();
    assert_eq!((report.inserted, report.updated), (0, 1));

    db.with_conn(|c| {
        let rows = list_regions(c, false)?;
        let bueno = rows
            .iter()
            .find(|r| r.slug == "br-go-goiania-setor-bueno")
            .unwrap();
        assert!(
            !bueno.enabled,
            "a region the user disabled must stay disabled"
        );
        assert!(
            rows.iter().any(|r| r.slug == "custom-user-region"),
            "user regions are never deleted"
        );
        let marista = rows
            .iter()
            .find(|r| r.slug == "br-go-goiania-setor-marista")
            .unwrap();
        assert_eq!(marista.name, "Setor Marista (renomeado)");
        assert_eq!(list_regions(c, true)?.len(), rows.len() - 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn seed_can_add_a_new_region_without_touching_existing_ones() {
    let db = seeded();
    let seed = parse_seed(&seed_json(
        r#"{ "slug": "br-go-goiania-setor-oeste", "state_code": "GO", "municipality": "Goiânia",
             "name": "Setor Oeste", "region_type": "neighborhood", "parent": "br-go-goiania" },
           { "slug": "br-go-goiania", "state_code": "GO", "municipality": "Goiânia",
             "name": "Goiânia", "region_type": "municipality", "parent": "br-go" },
           { "slug": "br-go", "state_code": "GO", "name": "Goiás", "region_type": "state" }"#,
    ))
    .unwrap();
    let report = db.with_conn(|c| seed_regions(c, &seed)).unwrap();
    assert_eq!(
        (report.inserted, report.updated, report.unchanged),
        (1, 0, 2)
    );
}

const STATE_JSON: &str =
    r#"{ "slug": "br-go", "state_code": "GO", "name": "Goiás", "region_type": "state" }"#;
const CITY_JSON: &str = r#"{ "slug": "br-go-x", "state_code": "GO", "municipality": "X", "name": "X", "region_type": "municipality", "parent": "br-go" }"#;

/// Seeds that are wrong on their own: bad syntax, unknown or invalid fields.
fn malformed_seeds() -> Vec<(&'static str, String)> {
    vec![
        ("not json", "nope".into()),
        ("wrong version", r#"{ "version": 2, "regions": [] }"#.into()),
        (
            "unknown top-level field",
            r#"{ "version": 1, "regions": [], "extra": 1 }"#.into(),
        ),
        (
            "unknown region field",
            seed_json(
                r#"{ "slug": "a", "state_code": "GO", "name": "A", "region_type": "state", "oops": 1 }"#,
            ),
        ),
        (
            "unknown region type",
            seed_json(
                r#"{ "slug": "a", "state_code": "GO", "name": "A", "region_type": "galaxy" }"#,
            ),
        ),
        (
            "duplicate slug",
            seed_json(&format!("{STATE_JSON},{STATE_JSON}")),
        ),
        (
            "slug with spaces",
            seed_json(
                r#"{ "slug": "has space", "state_code": "GO", "name": "A", "region_type": "state" }"#,
            ),
        ),
        (
            "empty slug",
            seed_json(r#"{ "slug": "", "state_code": "GO", "name": "A", "region_type": "state" }"#),
        ),
        (
            "bad state code",
            seed_json(r#"{ "slug": "a", "state_code": "G", "name": "A", "region_type": "state" }"#),
        ),
        (
            "lowercase state code",
            seed_json(
                r#"{ "slug": "a", "state_code": "go", "name": "A", "region_type": "state" }"#,
            ),
        ),
        (
            "empty name",
            seed_json(
                r#"{ "slug": "a", "state_code": "GO", "name": "  ", "region_type": "state" }"#,
            ),
        ),
        (
            "state with a municipality",
            seed_json(
                r#"{ "slug": "a", "state_code": "GO", "municipality": "X", "name": "A", "region_type": "state" }"#,
            ),
        ),
        (
            "metadata that is not an object",
            seed_json(
                r#"{ "slug": "a", "state_code": "GO", "name": "A", "region_type": "state", "metadata": [1] }"#,
            ),
        ),
    ]
}

/// Seeds whose regions are individually fine but inconsistent with each other: parents,
/// municipalities and names.
fn inconsistent_seeds() -> Vec<(&'static str, String)> {
    vec![
        (
            "municipality without municipality",
            seed_json(&format!(
                "{STATE_JSON},{}",
                r#"{ "slug": "b", "state_code": "GO", "name": "B", "region_type": "municipality", "parent": "br-go" }"#
            )),
        ),
        (
            "unknown parent",
            seed_json(
                r#"{ "slug": "a", "state_code": "GO", "municipality": "X", "name": "A", "region_type": "municipality", "parent": "ghost" }"#,
            ),
        ),
        (
            "self parent",
            seed_json(
                r#"{ "slug": "a", "state_code": "GO", "municipality": "X", "name": "A", "region_type": "municipality", "parent": "a" }"#,
            ),
        ),
        (
            "parent cycle",
            seed_json(
                r#"{ "slug": "a", "state_code": "GO", "municipality": "X", "name": "A", "region_type": "neighborhood", "parent": "b" },
               { "slug": "b", "state_code": "GO", "municipality": "X", "name": "B", "region_type": "neighborhood", "parent": "a" }"#,
            ),
        ),
        (
            "municipality under a municipality",
            seed_json(&format!(
                "{STATE_JSON},{CITY_JSON},{}",
                r#"{ "slug": "br-go-y", "state_code": "GO", "municipality": "Y", "name": "Y", "region_type": "municipality", "parent": "br-go-x" }"#
            )),
        ),
        (
            "neighbourhood under a state",
            seed_json(&format!(
                "{STATE_JSON},{}",
                r#"{ "slug": "n", "state_code": "GO", "municipality": "X", "name": "N", "region_type": "neighborhood", "parent": "br-go" }"#
            )),
        ),
        (
            "neighbourhood in another municipality than its parent",
            seed_json(&format!(
                "{STATE_JSON},{CITY_JSON},{}",
                r#"{ "slug": "n", "state_code": "GO", "municipality": "Z", "name": "N", "region_type": "neighborhood", "parent": "br-go-x" }"#
            )),
        ),
        (
            "empty alias",
            seed_json(&format!(
                "{STATE_JSON},{CITY_JSON},{}",
                r#"{ "slug": "n", "state_code": "GO", "municipality": "X", "name": "N", "region_type": "neighborhood", "parent": "br-go-x", "aliases": [" "] }"#
            )),
        ),
        (
            "two neighbourhoods of one municipality sharing a name",
            seed_json(&format!(
                "{STATE_JSON},{CITY_JSON},{},{}",
                r#"{ "slug": "n1", "state_code": "GO", "municipality": "X", "name": "Centro", "region_type": "neighborhood", "parent": "br-go-x" }"#,
                r#"{ "slug": "n2", "state_code": "GO", "municipality": "X", "name": "centro", "region_type": "neighborhood", "parent": "br-go-x" }"#
            )),
        ),
    ]
}

#[test]
fn invalid_seeds_are_rejected_before_anything_is_written() {
    let db = mem();
    let cases = malformed_seeds().into_iter().chain(inconsistent_seeds());
    for (description, json) in cases {
        let err = parse_seed(&json).expect_err(description);
        assert!(
            matches!(err, Error::InvalidConfig { .. } | Error::Json(_)),
            "{description}: unexpected error kind {err}"
        );
    }
    db.with_conn(|c| {
        let n: i64 = c.query_row("SELECT COUNT(*) FROM regions", [], |r| r.get(0))?;
        assert_eq!(n, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn every_region_type_is_accepted_by_the_database() {
    let db = mem();
    db.with_conn(|c| {
        for (i, t) in RegionType::ALL.iter().enumerate() {
            let municipality = (*t != RegionType::State).then_some("X");
            c.execute(
                "INSERT INTO regions (slug, state_code, municipality, name, region_type) VALUES (?1, 'GO', ?2, 'n', ?3)",
                rusqlite::params![format!("t{i}"), municipality, t.as_str()],
            )?;
        }
        Ok(())
    })
    .unwrap();
}

// ---- location resolution -------------------------------------------------------------------

#[test]
fn resolves_a_neighbourhood_with_its_municipality() {
    let db = seeded();
    let r = resolve(&db, "Casa no Setor Bueno, Goiânia - GO");
    assert_eq!(place(r.as_ref()), (Some("Goiânia"), Some("Setor Bueno")));
    let id = db
        .with_conn(|c| {
            Ok(list_regions(c, false)?
                .into_iter()
                .find(|r| r.name == "Setor Bueno")
                .unwrap()
                .id)
        })
        .unwrap();
    assert_eq!(r.unwrap().region_id, id);
}

#[test]
fn matching_ignores_case_and_accents_and_understands_aliases() {
    let db = seeded();
    assert_eq!(
        place(resolve(&db, "SETOR MARISTA").as_ref()),
        (Some("Goiânia"), Some("Setor Marista"))
    );
    assert_eq!(
        place(resolve(&db, "jardim goias").as_ref()),
        (Some("Goiânia"), Some("Jardim Goiás"))
    );
    assert_eq!(
        place(resolve(&db, "Apto no Jd. Goiás").as_ref()),
        (Some("Goiânia"), Some("Jardim Goiás"))
    );
    assert_eq!(
        place(resolve(&db, "St. Bueno").as_ref()),
        (Some("Goiânia"), Some("Setor Bueno"))
    );
}

#[test]
fn a_neighbourhood_alone_implies_its_municipality() {
    let db = seeded();
    assert_eq!(
        place(resolve(&db, "Garavelo").as_ref()),
        (Some("Aparecida de Goiânia"), Some("Garavelo"))
    );
    assert_eq!(
        place(resolve(&db, "Cidade Livre - Aparecida de Goiânia").as_ref()),
        (Some("Aparecida de Goiânia"), Some("Cidade Livre"))
    );
}

#[test]
fn a_municipality_alone_resolves_to_the_municipality_and_the_longer_name_wins() {
    let db = seeded();
    assert_eq!(
        place(resolve(&db, "Goiânia").as_ref()),
        (Some("Goiânia"), None)
    );
    // "Aparecida de Goiânia" contains "Goiânia"; it must not be read as Goiânia.
    assert_eq!(
        place(resolve(&db, "Aparecida de Goiânia").as_ref()),
        (Some("Aparecida de Goiânia"), None)
    );
    assert_eq!(
        place(resolve(&db, "Hidrolândia").as_ref()),
        (Some("Hidrolândia"), None)
    );
}

#[test]
fn contradicting_location_text_resolves_to_nothing() {
    let db = seeded();
    // Setor Bueno is in Goiânia, but the text claims Aparecida.
    assert_eq!(resolve(&db, "Setor Bueno, Aparecida de Goiânia"), None);
}

#[test]
fn state_names_and_partial_words_do_not_resolve() {
    let db = seeded();
    assert_eq!(resolve(&db, "Goiás"), None);
    assert_eq!(resolve(&db, "Brasília"), None);
    assert_eq!(resolve(&db, "Garavelos"), None);
    assert_eq!(resolve(&db, ""), None);
    assert_eq!(resolve(&db, "Centro de São Paulo"), None);
}

#[test]
fn a_corridor_needs_its_municipality_in_the_text() {
    let db = seeded();
    let r = resolve(&db, "Chácara na BR-153 Hidrolândia");
    assert_eq!(
        place(r.as_ref()),
        (Some("Hidrolândia"), Some("Corredor BR-153 (Hidrolândia)"))
    );
    // Goiânia and Aparecida are on BR-153 too: the highway alone says nothing.
    assert_eq!(resolve(&db, "Chácara na BR-153"), None);
}

#[test]
fn disabled_regions_are_ignored() {
    let db = seeded();
    db.with_conn(|c| {
        c.execute(
            "UPDATE regions SET enabled = 0 WHERE slug = 'br-go-goiania-setor-bueno'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        place(resolve(&db, "Setor Bueno, Goiânia").as_ref()),
        (Some("Goiânia"), None)
    );
}

#[test]
fn identical_neighbourhood_names_in_two_municipalities_are_ambiguous_without_the_municipality() {
    let db = seeded();
    db.with_conn(|c| {
        for (slug, municipality, parent) in [
            ("centro-goiania", "Goiânia", "br-go-goiania"),
            ("centro-aparecida", "Aparecida de Goiânia", "br-go-aparecida-de-goiania"),
        ] {
            c.execute(
                "INSERT INTO regions (slug, state_code, municipality, name, region_type, parent_id)
                 VALUES (?1, 'GO', ?2, 'Centro', 'neighborhood', (SELECT id FROM regions WHERE slug = ?3))",
                rusqlite::params![slug, municipality, parent],
            )?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(resolve(&db, "Casa no Centro"), None);
    assert_eq!(
        place(resolve(&db, "Centro, Goiânia").as_ref()),
        (Some("Goiânia"), Some("Centro"))
    );
    assert_eq!(
        place(resolve(&db, "Centro, Aparecida de Goiânia").as_ref()),
        (Some("Aparecida de Goiânia"), Some("Centro"))
    );
}
