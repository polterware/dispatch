#![deny(unsafe_code)]

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/dispatch.v1.rs"));
    include!(concat!(env!("OUT_DIR"), "/dispatch.v1.serde.rs"));

    pub mod host {
        include!(concat!(env!("OUT_DIR"), "/polterware.local.v1.rs"));
        include!(concat!(env!("OUT_DIR"), "/polterware.local.v1.serde.rs"));
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub mod catalog;
pub mod config;
pub mod difficulty;
pub mod pass_model;
pub mod roles;
pub mod route_decision;
pub mod router;
pub mod tiers;
pub mod tiers_build;

pub mod adapters;
pub mod classifier;
pub mod cli;
pub mod coreclient;
pub mod hook;
pub mod host;
pub mod process;
pub mod reports;
pub mod runner;
pub mod runstate;
pub mod sandbox;
pub mod server;
pub mod setup;
pub mod skill;
pub mod store;
pub mod types;
pub mod usage;
pub mod worktree;
