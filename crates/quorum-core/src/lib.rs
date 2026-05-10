//! Quorum core — review pipeline, bundle assembly, JSON archive.
//!
//! This crate is fully synchronous. It does NOT depend on tokio, reqwest,
//! or `quorum-lippa-client`. Cross-crate calls go through public APIs only.

pub mod archive;
pub mod bundle;
pub mod config;
pub mod conventions;
pub mod deny_list;
pub mod discovery;
pub mod git;
pub mod review;

pub use review::{
    review_from_json, Finding, FindingSource, ParseError, RepoMetadata, Review, Severity,
};
