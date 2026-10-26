//! Deterministic text parsing for Brazilian Portuguese real-estate advertisements.

pub mod br_real_estate;
mod number;
mod text;

pub use br_real_estate::{PARSER_VERSION, parse_listing_text};
pub use number::parse_money;
