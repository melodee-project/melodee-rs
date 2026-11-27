//! Filesystem custody building blocks.
//!
//! The provisional lock scope in this module does not establish that every
//! configured media root is disjoint, that old writers have stopped, or that
//! a database reservation is live. It is not authority to remove media or
//! acknowledge a database page. Those checks must be incorporated before any
//! destructive operation is exposed.

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod lifecycle;

#[cfg(target_os = "linux")]
mod file_preflight;

#[cfg(target_os = "linux")]
mod maintenance;

#[cfg(target_os = "linux")]
mod native_path;

#[cfg(target_os = "linux")]
mod root_set;

#[cfg(all(test, target_os = "linux"))]
mod archive;

#[cfg(all(test, target_os = "linux"))]
mod quarantine;

#[cfg(target_os = "linux")]
pub use lifecycle::{HeldLifecycleLocks, LifecycleLockIdentity};

#[cfg(target_os = "linux")]
pub use file_preflight::{ExpectedFile, ExpectedHash, preflight_file_page};

#[cfg(target_os = "linux")]
pub use file_preflight::{IncrementalFileCheck, IncrementalFileProgress, ObservationScope};

#[cfg(target_os = "linux")]
pub use maintenance::{MaintenanceCredentials, MaintenanceObservationError};

#[cfg(target_os = "linux")]
pub use native_path::{decode_native_root, encode_native_root};

#[cfg(target_os = "linux")]
pub use root_set::{ConfiguredRoot, ExactConfiguredRootSet, RootExpectation, VerifiedRootSet};
