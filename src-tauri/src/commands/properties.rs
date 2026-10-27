//! Saved results: the paginated list with its filters, and the user's favorite/hidden choice.

// Tauri hands `State` to commands by value.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use serde::Deserialize;
use tauri::State;

use super::validate_slug;
use crate::{
    db::properties::{self as store, Page, PropertyQuery, PropertyView},
    error::Result,
    models::{listing::UserStatus, money::Money},
    service::AppService,
};

/// Filters and paging of the saved-results list. Mirrored by `ListPropertiesArgs` in
/// `src/lib/api.ts`. The default is every visible property.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ListPropertiesArgs {
    pub include_hidden: bool,
    pub favorites_only: bool,
    /// A region slug; matches that region and everything below it.
    pub region_slug: Option<String>,
    pub min_price_centavos: Option<i64>,
    pub max_price_centavos: Option<i64>,
    /// "Ocultar Ágio (Apenas Valor Total)": only properties with an established full price.
    pub full_price_only: bool,
    /// `0` means the default page size; larger values are capped.
    pub limit: u32,
    pub offset: u32,
}

impl ListPropertiesArgs {
    pub fn into_query(self) -> Result<PropertyQuery> {
        if let Some(slug) = &self.region_slug {
            validate_slug(slug)?;
        }
        Ok(PropertyQuery {
            include_hidden: self.include_hidden,
            favorites_only: self.favorites_only,
            region_slug: self.region_slug,
            min_price: self
                .min_price_centavos
                .map(Money::from_centavos)
                .transpose()?,
            max_price: self
                .max_price_centavos
                .map(Money::from_centavos)
                .transpose()?,
            full_price_only: self.full_price_only,
            limit: self.limit,
            offset: self.offset,
        })
    }
}

#[tauri::command]
pub async fn list_properties(
    service: State<'_, Arc<AppService>>,
    args: ListPropertiesArgs,
) -> Result<Page<PropertyView>> {
    let query = args.into_query()?;
    service
        .blocking(move |conn| store::list_properties(conn, &query))
        .await
}

/// Applies the choice to the property's whole merge group. Returns how many property rows were
/// touched (0 when it does not exist).
#[tauri::command]
pub async fn set_property_status(
    service: State<'_, Arc<AppService>>,
    property_id: i64,
    status: UserStatus,
) -> Result<usize> {
    let at = chrono::Utc::now();
    service
        .blocking(move |conn| store::set_user_status(conn, property_id, status, at))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn args(json: &str) -> ListPropertiesArgs {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_default_is_every_visible_property_on_the_first_page() {
        let query = args("{}").into_query().unwrap();
        assert!(!query.include_hidden && !query.favorites_only && !query.full_price_only);
        assert!(
            query.region_slug.is_none() && query.min_price.is_none() && query.max_price.is_none()
        );
        assert_eq!(
            (query.effective_limit(), query.offset),
            (PropertyQuery::DEFAULT_LIMIT, 0)
        );
    }

    #[test]
    fn every_filter_reaches_the_query_in_exact_units() {
        let query = args(
            r#"{ "includeHidden": true, "favoritesOnly": true, "regionSlug": "br-go-goiania",
                 "minPriceCentavos": 30000000, "maxPriceCentavos": 45000000,
                 "fullPriceOnly": true, "limit": 25, "offset": 50 }"#,
        )
        .into_query()
        .unwrap();
        assert!(query.include_hidden && query.favorites_only && query.full_price_only);
        assert_eq!(query.region_slug.as_deref(), Some("br-go-goiania"));
        assert_eq!(query.min_price.map(Money::centavos), Some(30_000_000));
        assert_eq!(query.max_price.map(Money::centavos), Some(45_000_000));
        assert_eq!((query.limit, query.offset), (25, 50));
    }

    #[test]
    fn the_page_size_is_capped() {
        assert_eq!(
            args(r#"{ "limit": 100000 }"#)
                .into_query()
                .unwrap()
                .effective_limit(),
            PropertyQuery::MAX_LIMIT
        );
    }

    #[test]
    fn malformed_filters_are_refused() {
        let error = args(r#"{ "regionSlug": "Setor Bueno" }"#)
            .into_query()
            .unwrap_err();
        assert!(matches!(error, Error::InvalidConfig { .. }), "{error}");
        let error = args(r#"{ "minPriceCentavos": -5 }"#)
            .into_query()
            .unwrap_err();
        assert!(matches!(error, Error::Model(_)), "{error}");
        assert!(
            serde_json::from_str::<ListPropertiesArgs>(r#"{ "fullPrice": true }"#).is_err(),
            "unknown field"
        );
    }

    #[test]
    fn a_user_status_is_read_from_its_stored_text_form() {
        for (text, status) in [
            ("new", UserStatus::New),
            ("favorite", UserStatus::Favorite),
            ("hidden", UserStatus::Hidden),
        ] {
            assert_eq!(
                serde_json::from_str::<UserStatus>(&format!("\"{text}\"")).unwrap(),
                status
            );
        }
        assert!(serde_json::from_str::<UserStatus>("\"archived\"").is_err());
    }
}
