//! The Quorum benchmark harness (v0.6 Stage 1, spec §4). Not shipped: the
//! crate is `publish = false`, `dist = false`, and no published crate
//! depends on it. It drives the installed `quorum` binary and never links
//! Quorum's libraries.

pub mod archive;
pub mod budget;
pub mod claims;
pub mod config;
pub mod corpus;
pub mod fake_lippa;
pub mod findings;
pub mod keyguard;
pub mod manifest;
pub mod materialize;
pub mod preflight;
pub mod prices;
pub mod proxy;
pub mod quorum_run;
pub mod runid;
pub mod scrub;
pub mod secret;
pub mod vendors;
pub mod verify;
