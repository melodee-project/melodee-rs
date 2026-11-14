//! Shared server and CLI use cases, independent of HTTP protocol adapters.
#![forbid(unsafe_code)]

pub mod avatars;
pub mod correlation;
pub mod notifications;
pub mod onboarding;
pub mod podcast_worker;
pub mod resource_bounds;
pub mod scripting;
pub mod theme_archive;
