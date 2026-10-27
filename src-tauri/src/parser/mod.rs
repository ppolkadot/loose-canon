//! Deterministic text parsing for Brazilian Portuguese real-estate advertisements.

pub mod br_real_estate;
mod number;
mod text;

pub use br_real_estate::{
    PARSER_VERSION, PriceFieldHint, StructuredText, parse_listing_text,
    parse_listing_with_price_hint, parse_structured_text,
};
pub use number::parse_money;
pub use text::fold_text;
