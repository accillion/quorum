//! Doc anchor — the dismiss-prompt logic lives in [`super::state`] so
//! it can be unit-tested without a terminal. This module is kept so
//! v1.0 §4.1 (which lists `dismiss_prompt.rs` as a top-level module
//! file) is honored at the path level.
//!
//! See:
//!   - [`super::state::Modal::DismissReason`] — the reason-picker modal
//!     state.
//!   - [`super::state::Modal::DismissNote`] — the free-text note input
//!     for `other` reason; rules per spec §4.3.3 (≤2KB, control chars
//!     stripped except `\t`→space, embedded newlines rejected, empty
//!     rejected).
//!   - [`super::panels::render`] — modal rendering branches that paint
//!     the prompts.
