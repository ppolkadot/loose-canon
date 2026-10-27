//! Local image processing: perceptual hashing, near-duplicate lookup, bounded decoding and the
//! on-disk cache. Everything runs locally; the hash only ever proposes match *candidates*.

pub mod cache;
pub mod decode;
pub mod index;
pub mod phash;
pub mod pipeline;
