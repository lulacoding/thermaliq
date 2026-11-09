//! Daemon wiring: paths, config, history, API, service, agent. See SPEC.md §5.

#[cfg(windows)]
pub mod acl;
pub mod agent;
pub mod alerts;
pub mod anti_cheat;
pub mod api;
pub mod app;
pub mod arbiter;
pub mod decision_client;
pub mod foreground;
pub mod health;
pub mod history;
pub mod identity;
#[cfg(feature = "gate-tests")]
pub mod inject;
pub mod keychain;
#[cfg(windows)]
pub mod lhm_bridge;
pub mod lifecycle;
pub mod runtime;
#[cfg(windows)]
pub mod service;
pub mod slow;
pub mod store;
#[cfg(windows)]
#[allow(unsafe_code)]
mod win;
