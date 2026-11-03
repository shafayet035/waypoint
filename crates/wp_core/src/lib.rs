//! Foundation types shared by every Waypoint crate.
//!
//! - [`diag`]: diagnostics with stable codes, rendered for humans and as JSON.
//! - [`time`]: the fixed-timestep simulation clock.
//! - [`rng`]: a seeded, platform-independent random number generator.
//! - [`hash`]: a stable hash for world-state fingerprints.

pub mod diag;
pub mod hash;
pub mod rng;
pub mod time;

pub use diag::{Diagnostic, Diagnostics, Location, Severity};
pub use glam;
pub use hash::StableHasher;
pub use rng::SimRng;
pub use time::SimTime;
