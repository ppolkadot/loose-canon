//! URL normalisation: the identity of an advertisement is `(adapter id, normalised URL)`.
//!
//! The rules are deliberately conservative. Normalisation may remove noise (fragments, tracking
//! parameters, parameter order, default ports) but it must never merge two *different*
//! advertisements: every parameter that is not a well-known tracking parameter is kept, and path
//! text (including a trailing slash) is left exactly as given.

use url::Url;

/// Longest URL accepted. Longer ones are almost certainly not listing pages.
pub const MAX_URL_LENGTH: usize = 2048;

/// Parameters that only identify a campaign or a click, never a page. Matched case-insensitively.
const TRACKING_PARAMS: &[&str] = &[
    "fbclid", "gclid", "dclid", "msclkid", "yclid", "mc_cid", "mc_eid", "_ga", "igshid",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("not an absolute URL: {0}")]
    Invalid(String),
    #[error("only http and https URLs are supported (got {0:?})")]
    UnsupportedScheme(String),
    #[error("URLs carrying credentials are refused")]
    HasCredentials,
    #[error("URL is longer than {MAX_URL_LENGTH} characters")]
    TooLong,
}

fn is_tracking(name: &str, extra_ignored: &[String]) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("utm_")
        || TRACKING_PARAMS.contains(&lower.as_str())
        || extra_ignored.iter().any(|e| e.eq_ignore_ascii_case(name))
}

/// Parses and validates `raw` without changing it (scheme, credentials, length).
pub fn parse_http_url(raw: &str) -> Result<Url, UrlError> {
    if raw.len() > MAX_URL_LENGTH {
        return Err(UrlError::TooLong);
    }
    let url =
        Url::parse(raw.trim()).map_err(|_| UrlError::Invalid(raw.chars().take(80).collect()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UrlError::UnsupportedScheme(url.scheme().to_owned()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlError::HasCredentials);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(UrlError::Invalid(raw.chars().take(80).collect()));
    }
    Ok(url)
}

/// The canonical form used to recognise the same advertisement when it is seen again.
/// `extra_ignored_params` are additional per-source tracking parameters.
pub fn normalize_url(raw: &str, extra_ignored_params: &[String]) -> Result<String, UrlError> {
    let mut url = parse_http_url(raw)?;
    url.set_fragment(None);

    let mut kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| !is_tracking(name, extra_ignored_params))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    kept.sort();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(&kept);
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(raw: &str) -> String {
        normalize_url(raw, &[]).unwrap_or_else(|e| panic!("{raw}: {e}"))
    }

    #[test]
    fn removes_fragments_tracking_parameters_and_default_ports() {
        assert_eq!(
            n("https://Example.COM:443/imovel/123?utm_source=x&UTM_Medium=y&fbclid=abc&id=7#fotos"),
            "https://example.com/imovel/123?id=7"
        );
        assert_eq!(n("http://example.com:80/a"), "http://example.com/a");
        assert_eq!(n("https://example.com"), "https://example.com/");
    }

    #[test]
    fn parameter_order_does_not_create_a_second_identity() {
        assert_eq!(n("https://e.test/s?b=2&a=1"), n("https://e.test/s?a=1&b=2"));
        assert_eq!(n("https://e.test/s?b=2&a=1"), "https://e.test/s?a=1&b=2");
    }

    #[test]
    fn meaningful_parameters_and_paths_keep_distinct_advertisements_distinct() {
        assert_ne!(
            n("https://e.test/imovel?id=1"),
            n("https://e.test/imovel?id=2")
        );
        assert_ne!(n("https://e.test/imovel/1"), n("https://e.test/imovel/2"));
        assert_ne!(
            n("https://e.test/imovel/1"),
            n("https://e.test/imovel/1/"),
            "path text is not rewritten"
        );
        assert_ne!(n("https://a.test/imovel/1"), n("https://b.test/imovel/1"));
        assert_ne!(n("http://e.test/imovel/1"), n("https://e.test/imovel/1"));
        // Repeated parameters are kept, not collapsed.
        assert_eq!(n("https://e.test/s?x=2&x=1"), "https://e.test/s?x=1&x=2");
    }

    #[test]
    fn per_source_tracking_parameters_are_removed_case_insensitively() {
        let extra = vec!["ref".to_owned(), "Source".to_owned()];
        assert_eq!(
            normalize_url("https://e.test/a?REF=home&source=app&id=3", &extra).unwrap(),
            "https://e.test/a?id=3"
        );
    }

    #[test]
    fn is_idempotent() {
        for raw in [
            "https://Example.com/a?b=2&a=1&utm_x=1#f",
            "http://e.test:8080/p?q=%C3%A1gua",
            "https://e.test/",
        ] {
            let once = n(raw);
            assert_eq!(n(&once), once, "{raw}");
        }
    }

    #[test]
    fn keeps_percent_encoded_values_intact() {
        assert_eq!(
            n("https://e.test/s?q=%C3%A1gua&a=1"),
            "https://e.test/s?a=1&q=%C3%A1gua"
        );
    }

    #[test]
    fn refuses_anything_that_is_not_a_plain_http_url() {
        for (raw, expected) in [
            ("ftp://e.test/a", UrlError::UnsupportedScheme("ftp".into())),
            (
                "javascript:alert(1)",
                UrlError::UnsupportedScheme("javascript".into()),
            ),
            (
                "file:///etc/passwd",
                UrlError::UnsupportedScheme("file".into()),
            ),
            ("https://user:pw@e.test/a", UrlError::HasCredentials),
            ("https://user@e.test/a", UrlError::HasCredentials),
        ] {
            assert_eq!(normalize_url(raw, &[]).unwrap_err(), expected, "{raw}");
        }
        for raw in ["/relative/path", "not a url", "", "https://", "//e.test/a"] {
            assert!(
                matches!(normalize_url(raw, &[]), Err(UrlError::Invalid(_))),
                "{raw:?}"
            );
        }
        let long = format!("https://e.test/{}", "a".repeat(MAX_URL_LENGTH));
        assert_eq!(normalize_url(&long, &[]).unwrap_err(), UrlError::TooLong);
    }

    #[test]
    fn tolerates_surrounding_whitespace() {
        assert_eq!(n("  https://e.test/a  "), "https://e.test/a");
    }
}
