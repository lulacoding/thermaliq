//! Daemon wiring: paths, config, history, API, service, agent. See SPEC.md §5.

#[cfg(windows)]
pub mod acl;
pub mod api;
pub mod app;
pub mod history;
pub mod runtime;
#[cfg(windows)]
pub mod service;
pub mod store;
