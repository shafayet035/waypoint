//! Waypoint's entity-component system.
//!
//! Built for agents and determinism:
//! - Entity IDs are allocated deterministically and carry a generation.
//! - Component storage iterates in a deterministic order (no hash-map iteration).
//! - Every component is serde-serializable, so the control protocol, save files and
//!   state hashes work for any component, including ones a game defines.
//! - Systems run in explicit stages with a stable order; `Schedule::describe`
//!   prints the resolved graph (`wp systems`).
//!
//! ```
//! use wp_ecs::{Component, World};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct Speed(f32);
//! impl Component for Speed { const NAME: &'static str = "Speed"; }
//!
//! let mut world = World::new();
//! let e = world.spawn();
//! world.insert(e, Speed(3.0));
//! world.query::<&mut Speed>().for_each(|_, s| s.0 *= 2.0);
//! assert_eq!(world.get::<Speed>(e).unwrap().0, 6.0);
//! ```

mod entity;
mod events;
mod query;
mod schedule;
mod storage;
mod world;

pub use entity::Entity;
pub use events::Events;
pub use query::{Query, QueryParam};
pub use schedule::{Schedule, ScheduleError, Stage, SystemBuilder, SystemInfo};
#[doc(hidden)]
pub use storage::Storage;
pub use world::World;

use serde::{Serialize, de::DeserializeOwned};

/// Data attached to an entity. `NAME` is the stable name used by reflection,
/// the control protocol and save files; never rename it once shipped.
pub trait Component: 'static + Serialize + DeserializeOwned {
    const NAME: &'static str;
}
