//! Text encoding, re-exported.
//!
//! This module used to hold the encoding policy. It now lives in
//! `rclip_core::text`, next to the registry that `plan/PLAN.md` §4.1 says owns
//! the `text/html` trap, and this is a re-export so the facade's own call sites
//! did not all have to move at once.

#[cfg(feature = "html")]
pub(crate) use rclip_core::text::decode_html_bytes;
pub(crate) use rclip_core::text::{decode_plain, decode_utf8_lossy, encode_plain};
