//! Road networks.
//!
//! - [`def`]: the `.road.ron` format (nodes, roads, lanes, shapes) and validation.
//! - [`geometry`]: polylines and centreline shapes (straight, arc, curve).
//! - [`network`]: the built network — trimmed roads, lanes, junction
//!   connectors (by turn rules) and A* routing.
//! - [`mesh`]: drawable geometry (asphalt, markings, sidewalks) as plain data.
//!
//! Everything here is pure: build a [`RoadNetwork`] once from a definition and
//! read it. Scenes reference road files; `wp_world` inserts the built network
//! as a resource.

pub mod def;
pub mod geometry;
pub mod mesh;
pub mod network;
pub mod opendrive;
pub mod osm;

pub use def::{Control, DriveSide, NodeDef, RoadDef, RoadNetworkDef, RoadShape, SignalTiming};
pub use geometry::Polyline;
pub use mesh::{Mesh, RoadMeshes, build_meshes};
pub use network::{
    Junction, Lane, LaneId, LaneKind, Light, RoadGeometry, RoadNetwork, Taper, Turn, taper_length_m,
};
