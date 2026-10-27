//! Region configuration: a validated, extensible seed file loaded into `regions`, plus
//! conservative resolution of a listing's raw location text to a region.
//!
//! Seeding is idempotent and non-destructive: regions are matched by their stable `slug`, a
//! changed definition is updated, regions absent from the seed are left alone, and the
//! user-controlled `enabled` flag of an existing region is never overwritten.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    models::region::RegionType,
    parser::fold_text,
};

/// Initial coverage (Goiânia, Aparecida de Goiânia, Hidrolândia). Embedded so the app does not
/// depend on a file path; adding regions is editing this file or inserting rows.
pub const DEFAULT_SEED_JSON: &str = include_str!("../../data/regions.json");

const SUPPORTED_SEED_VERSION: u32 = 1;
/// Depth bound when walking parent links, so a malformed seed cannot loop forever.
const MAX_HIERARCHY_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionSeed {
    pub version: u32,
    pub regions: Vec<SeedRegion>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedRegion {
    pub slug: String,
    pub state_code: String,
    #[serde(default)]
    pub municipality: Option<String>,
    pub name: String,
    pub region_type: RegionType,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SeedReport {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionRow {
    pub id: i64,
    pub slug: String,
    pub state_code: String,
    pub municipality: Option<String>,
    pub name: String,
    pub region_type: RegionType,
    pub parent_id: Option<i64>,
    pub enabled: bool,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLocation {
    pub region_id: i64,
    pub municipality: Option<String>,
    pub neighborhood: Option<String>,
}

fn invalid(detail: impl Into<String>) -> Error {
    Error::InvalidConfig {
        what: "region seed",
        detail: detail.into(),
    }
}

/// Parses and **validates** a seed (everything checkable without a database).
pub fn parse_seed(json: &str) -> Result<RegionSeed> {
    let seed: RegionSeed = serde_json::from_str(json)?;
    validate(&seed)?;
    Ok(seed)
}

fn validate(seed: &RegionSeed) -> Result<()> {
    if seed.version != SUPPORTED_SEED_VERSION {
        return Err(invalid(format!(
            "version {} is not supported (expected {SUPPORTED_SEED_VERSION})",
            seed.version
        )));
    }
    let mut by_slug: HashMap<&str, &SeedRegion> = HashMap::new();
    for region in &seed.regions {
        validate_fields(region)?;
        if by_slug.insert(region.slug.as_str(), region).is_some() {
            return Err(invalid(format!("duplicate slug {:?}", region.slug)));
        }
    }
    for region in &seed.regions {
        validate_parent(region, &by_slug)?;
    }
    validate_unique_names(seed)
}

fn validate_fields(region: &SeedRegion) -> Result<()> {
    let slug = &region.slug;
    let slug_ok = !slug.is_empty()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !slug_ok {
        return Err(invalid(format!(
            "slug {slug:?} must be non-empty lowercase ASCII letters, digits and hyphens"
        )));
    }
    let state_ok =
        region.state_code.len() == 2 && region.state_code.bytes().all(|b| b.is_ascii_uppercase());
    if !state_ok {
        return Err(invalid(format!(
            "{slug}: state_code must be two uppercase letters"
        )));
    }
    if region.name.trim().is_empty() {
        return Err(invalid(format!("{slug}: name is empty")));
    }
    if region.aliases.iter().any(|a| a.trim().is_empty()) {
        return Err(invalid(format!("{slug}: empty alias")));
    }
    match (
        region.region_type,
        region.municipality.as_deref().map(str::trim),
    ) {
        (RegionType::State, Some(_)) => {
            return Err(invalid(format!("{slug}: a state has no municipality")));
        }
        (RegionType::State, None) => {}
        (_, None | Some("")) => {
            return Err(invalid(format!("{slug}: municipality is required")));
        }
        _ => {}
    }
    if let Some(metadata) = &region.metadata {
        let Some(object) = metadata.as_object() else {
            return Err(invalid(format!("{slug}: metadata must be a JSON object")));
        };
        if object.contains_key("aliases") {
            return Err(invalid(format!(
                "{slug}: metadata.aliases is reserved; use `aliases`"
            )));
        }
    }
    Ok(())
}

fn validate_parent(region: &SeedRegion, by_slug: &HashMap<&str, &SeedRegion>) -> Result<()> {
    let slug = &region.slug;
    let Some(parent_slug) = region.parent.as_deref() else {
        if region.region_type == RegionType::State {
            return Ok(());
        }
        return Err(invalid(format!("{slug}: a parent is required")));
    };
    if region.region_type == RegionType::State {
        return Err(invalid(format!("{slug}: a state has no parent")));
    }
    if parent_slug == slug {
        return Err(invalid(format!(
            "{slug}: a region cannot be its own parent"
        )));
    }
    let Some(parent) = by_slug.get(parent_slug) else {
        return Err(invalid(format!("{slug}: unknown parent {parent_slug:?}")));
    };
    if parent.state_code != region.state_code {
        return Err(invalid(format!("{slug}: parent is in another state")));
    }
    let same_municipality = || {
        parent.municipality.as_deref().map(fold_text)
            == region.municipality.as_deref().map(fold_text)
    };
    match (region.region_type, parent.region_type) {
        (RegionType::Municipality, RegionType::State) => {}
        (RegionType::Municipality, _) => {
            return Err(invalid(format!(
                "{slug}: a municipality must sit directly under a state"
            )));
        }
        (_, RegionType::State) => {
            return Err(invalid(format!(
                "{slug}: a sub-municipal region cannot sit directly under a state"
            )));
        }
        (_, _) if !same_municipality() => {
            return Err(invalid(format!(
                "{slug}: municipality differs from its parent's"
            )));
        }
        _ => {}
    }
    // Walk to the root with a bound: a cycle never reaches one.
    let mut current = *parent;
    for _ in 0..MAX_HIERARCHY_DEPTH {
        let Some(next) = current.parent.as_deref() else {
            return Ok(());
        };
        if next == slug {
            return Err(invalid(format!("{slug}: parent cycle")));
        }
        let Some(next_region) = by_slug.get(next) else {
            return Err(invalid(format!("{slug}: unknown ancestor {next:?}")));
        };
        current = next_region;
    }
    Err(invalid(format!(
        "{slug}: hierarchy deeper than {MAX_HIERARCHY_DEPTH} levels or cyclic"
    )))
}

/// Two different regions must not answer to the same name inside the same scope, or the
/// resolver could not tell them apart.
fn validate_unique_names(seed: &RegionSeed) -> Result<()> {
    let mut claimed: HashMap<(bool, &str, String, String), &str> = HashMap::new();
    for region in &seed.regions {
        if region.region_type == RegionType::State {
            continue;
        }
        let sub = region.region_type.is_sub_municipal();
        let scope = if sub {
            fold_text(region.municipality.as_deref().unwrap_or_default())
        } else {
            String::new()
        };
        let names =
            std::iter::once(region.name.as_str()).chain(region.aliases.iter().map(String::as_str));
        for name in names {
            let key = (
                sub,
                region.state_code.as_str(),
                scope.clone(),
                fold_text(name),
            );
            if let Some(other) = claimed.insert(key, region.slug.as_str())
                && other != region.slug
            {
                return Err(invalid(format!(
                    "{:?} names both {other} and {} in the same scope",
                    name, region.slug
                )));
            }
        }
    }
    Ok(())
}

/// Order such that every parent precedes its children.
fn parents_first(seed: &RegionSeed) -> Vec<&SeedRegion> {
    let depth = |region: &SeedRegion| {
        let by_slug: HashMap<&str, &SeedRegion> =
            seed.regions.iter().map(|r| (r.slug.as_str(), r)).collect();
        let mut d = 0;
        let mut current = region;
        while let Some(parent) = current.parent.as_deref().and_then(|p| by_slug.get(p)) {
            d += 1;
            current = parent;
            if d > MAX_HIERARCHY_DEPTH {
                break;
            }
        }
        d
    };
    let mut ordered: Vec<&SeedRegion> = seed.regions.iter().collect();
    ordered.sort_by_key(|r| depth(r));
    ordered
}

fn metadata_json(region: &SeedRegion) -> Result<Option<String>> {
    let mut object = region
        .metadata
        .as_ref()
        .and_then(|m| m.as_object().cloned())
        .unwrap_or_default();
    if !region.aliases.is_empty() {
        object.insert("aliases".to_owned(), serde_json::json!(region.aliases));
    }
    if object.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(&object)?))
}

/// A region row as stored: `(id, state_code, municipality, name, region_type, parent_id, metadata_json)`.
type StoredRegion = (
    i64,
    String,
    Option<String>,
    String,
    String,
    Option<i64>,
    Option<String>,
);

/// Inserts or updates the seed's regions by `slug`, in one transaction. Never deletes rows and
/// never changes `enabled` of an existing region.
pub fn seed_regions(conn: &mut Connection, seed: &RegionSeed) -> Result<SeedReport> {
    validate(seed)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut report = SeedReport::default();
    let mut ids: HashMap<&str, i64> = HashMap::new();

    for region in parents_first(seed) {
        let parent_id = region
            .parent
            .as_deref()
            .map(|p| {
                ids.get(p)
                    .copied()
                    .ok_or_else(|| invalid(format!("unresolved parent {p:?}")))
            })
            .transpose()?;
        let metadata = metadata_json(region)?;
        let existing: Option<StoredRegion> = tx
            .query_row(
                "SELECT id, state_code, municipality, name, region_type, parent_id, metadata_json
                   FROM regions WHERE slug = ?1",
                [&region.slug],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let id = match existing {
            None => {
                tx.execute(
                    "INSERT INTO regions (slug, state_code, municipality, name, region_type, parent_id, metadata_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        region.slug,
                        region.state_code,
                        region.municipality,
                        region.name,
                        region.region_type.as_str(),
                        parent_id,
                        metadata
                    ],
                )?;
                report.inserted += 1;
                tx.last_insert_rowid()
            }
            Some((id, state, municipality, name, kind, parent, stored_metadata)) => {
                let same = state == region.state_code
                    && municipality == region.municipality
                    && name == region.name
                    && kind == region.region_type.as_str()
                    && parent == parent_id
                    && stored_metadata == metadata;
                if same {
                    report.unchanged += 1;
                } else {
                    tx.execute(
                        "UPDATE regions SET state_code = ?1, municipality = ?2, name = ?3,
                                region_type = ?4, parent_id = ?5, metadata_json = ?6
                          WHERE id = ?7",
                        params![
                            region.state_code,
                            region.municipality,
                            region.name,
                            region.region_type.as_str(),
                            parent_id,
                            metadata,
                            id
                        ],
                    )?;
                    report.updated += 1;
                }
                id
            }
        };
        ids.insert(region.slug.as_str(), id);
    }
    tx.commit()?;
    Ok(report)
}

fn aliases_of(metadata: Option<&str>) -> Vec<String> {
    metadata
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .and_then(|v| v.get("aliases").cloned())
        .and_then(|a| serde_json::from_value::<Vec<String>>(a).ok())
        .unwrap_or_default()
}

pub fn list_regions(conn: &Connection, only_enabled: bool) -> Result<Vec<RegionRow>> {
    let mut statement = conn.prepare(
        "SELECT id, slug, state_code, municipality, name, region_type, parent_id, enabled, metadata_json
           FROM regions
          WHERE (?1 = 0 OR enabled = 1)
          ORDER BY id",
    )?;
    let raw = statement
        .query_map([i64::from(only_enabled)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(
            |(id, slug, state_code, municipality, name, kind, parent_id, enabled, metadata)| {
                Ok(RegionRow {
                    id,
                    slug,
                    state_code,
                    municipality,
                    name,
                    region_type: kind.parse().map_err(|_| Error::InvalidStoredValue {
                        column: "regions.region_type",
                        value: kind.clone(),
                    })?,
                    parent_id,
                    enabled: enabled == 1,
                    aliases: aliases_of(metadata.as_deref()),
                })
            },
        )
        .collect()
}

struct IndexEntry {
    region_id: i64,
    region_type: RegionType,
    municipality: String,
    municipality_folded: String,
    display_name: String,
    folded_names: Vec<String>,
}

/// Name index over the enabled regions, built once and reused for many listings.
pub struct LocationIndex {
    entries: Vec<IndexEntry>,
}

impl LocationIndex {
    pub fn load(conn: &Connection) -> Result<Self> {
        let entries = list_regions(conn, true)?
            .into_iter()
            .filter(|r| r.region_type != RegionType::State)
            .map(|r| {
                let municipality = if r.region_type == RegionType::Municipality {
                    r.municipality.clone().unwrap_or_else(|| r.name.clone())
                } else {
                    r.municipality.clone().unwrap_or_default()
                };
                let folded_names = std::iter::once(r.name.as_str())
                    .chain(r.aliases.iter().map(String::as_str))
                    .map(fold_text)
                    .collect();
                IndexEntry {
                    region_id: r.id,
                    region_type: r.region_type,
                    municipality_folded: fold_text(&municipality),
                    municipality,
                    display_name: r.name,
                    folded_names,
                }
            })
            .collect();
        Ok(Self { entries })
    }

    /// The most specific region the text names, or `None` when nothing is named, the match is
    /// ambiguous, or the text contradicts itself (e.g. a neighbourhood of another municipality).
    #[must_use]
    pub fn resolve(&self, text: &str) -> Option<ResolvedLocation> {
        let folded = fold_text(text);
        let groups = self.matched_groups(&folded);

        let (municipality_groups, sub_groups): (Vec<_>, Vec<_>) = groups.iter().partition(|g| {
            g.iter()
                .all(|&i| self.entries[i].region_type == RegionType::Municipality)
        });

        let mentioned: HashSet<&str> = municipality_groups
            .iter()
            .flat_map(|g| g.iter())
            .map(|&i| self.entries[i].municipality_folded.as_str())
            .collect();
        if mentioned.len() > 1 {
            return None;
        }

        if sub_groups.is_empty() {
            let [group] = municipality_groups.as_slice() else {
                return None;
            };
            let [only] = group.as_slice() else {
                return None;
            };
            let entry = &self.entries[*only];
            return Some(ResolvedLocation {
                region_id: entry.region_id,
                municipality: Some(entry.municipality.clone()),
                neighborhood: None,
            });
        }

        let mut resolved: Vec<&IndexEntry> = Vec::new();
        for group in &sub_groups {
            let candidates: Vec<&IndexEntry> = group
                .iter()
                .map(|&i| &self.entries[i])
                .filter(|e| {
                    mentioned.is_empty() || mentioned.contains(e.municipality_folded.as_str())
                })
                .collect();
            match candidates.as_slice() {
                [only] => resolved.push(only),
                // Either contradicted by the stated municipality or still ambiguous.
                _ => return None,
            }
        }
        let [entry] = resolved.as_slice() else {
            return None;
        };
        Some(ResolvedLocation {
            region_id: entry.region_id,
            municipality: Some(entry.municipality.clone()),
            neighborhood: Some(entry.display_name.clone()),
        })
    }

    /// Entries naming the same stretch of text form one group. Longer names claim their text
    /// first, so "Aparecida de Goiânia" is never also read as "Goiânia".
    fn matched_groups(&self, folded: &str) -> Vec<Vec<usize>> {
        let is_word_char = |c: char| c.is_alphanumeric();
        let mut occurrences: Vec<(usize, usize, usize)> = Vec::new(); // (start, end, entry)
        for (index, entry) in self.entries.iter().enumerate() {
            for name in entry.folded_names.iter().filter(|n| !n.is_empty()) {
                for (start, _) in folded.match_indices(name.as_str()) {
                    let end = start + name.len();
                    let before_ok = folded[..start]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !is_word_char(c));
                    let after_ok = folded[end..]
                        .chars()
                        .next()
                        .is_none_or(|c| !is_word_char(c));
                    if before_ok && after_ok {
                        occurrences.push((start, end, index));
                    }
                }
            }
        }
        let mut spans: Vec<(usize, usize)> = occurrences.iter().map(|&(s, e, _)| (s, e)).collect();
        spans.sort_unstable();
        spans.dedup();
        spans.sort_by_key(|&(s, e)| (std::cmp::Reverse(e - s), s));

        let mut accepted: Vec<(usize, usize)> = Vec::new();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (start, end) in spans {
            if accepted.iter().any(|&(s, e)| start < e && s < end) {
                continue;
            }
            accepted.push((start, end));
            let mut group: Vec<usize> = occurrences
                .iter()
                .filter(|&&(s, e, _)| (s, e) == (start, end))
                .map(|&(_, _, i)| i)
                .collect();
            group.sort_unstable();
            group.dedup();
            groups.push(group);
        }
        groups
    }
}

#[cfg(test)]
mod tests;
