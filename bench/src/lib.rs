//! The Quorum benchmark harness (v0.6 Stage 1, spec §4). Not shipped: the
//! crate is `publish = false`, `dist = false`, and no published crate
//! depends on it. It drives the installed `quorum` binary and never links
//! Quorum's libraries.

pub mod adjudicate;
pub mod archive;
pub mod budget;
pub mod calls;
pub mod claims;
pub mod commands;
pub mod config;
pub mod corpus;
pub mod edit;
pub mod fake_lippa;
pub mod findings;
pub mod github;
pub mod keyguard;
pub mod manifest;
pub mod materialize;
pub mod pipeline;
pub mod preflight;
pub mod prices;
pub mod proxy;
pub mod quorum_run;
pub mod results;
pub mod runid;
pub mod runner;
pub mod score;
pub mod scrub;
pub mod secret;
pub mod stats;
pub mod templates;
pub mod vendors;
pub mod verify;
