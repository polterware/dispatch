//! The Polterware DispatchHostService client.

mod client;
mod types;

pub use client::{Client, DispatchHostServiceClient, default_db};
pub use types::{
    BoxFuture, CLIENT_HEADER, CLIENT_NAME, Card, CardsAndRepos, ERR_NO_HOST, Host, HostError,
    NoHost, NoneHost, Project, Repository, Run, is_no_host,
};
