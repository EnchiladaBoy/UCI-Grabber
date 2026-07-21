//! Safe, data-only installation primitives for complete UCI engine packages.

pub mod app;
pub mod catalog;
pub mod download;
pub mod error;
pub mod extract;
pub mod handoff;
pub mod install;
pub mod recipes;
pub mod registry;
pub mod schema;
pub mod uci;

pub use error::{Error, Result};
pub use install::{InstallOptions, Installer};
pub use registry::{InstallRecord, Registry, RegistryStore};
