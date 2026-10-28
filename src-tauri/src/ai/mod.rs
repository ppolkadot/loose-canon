//! The optional Gemini prioritisation of the best-ranked candidates.
//!
//! It is strictly supplementary: it runs only when the user enabled it and a key is stored, it
//! reads the local ranking and never writes to it, and every failure is reported as its own kind
//! while the local ranking stands. What is sent is a compact record per candidate with phone
//! numbers, e-mail addresses and links removed from the text; the key lives only in the Windows
//! credential store. Design: `docs/phase-3-design.md`.

pub mod candidate;
pub mod client;
pub mod failure;
pub mod prompt;
pub mod redact;
pub mod response;
pub mod secret;
pub mod settings;
