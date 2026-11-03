# wp_road

Road networks, all pure data and functions (no world access, no graphics).

- `def.rs`: the `.road.ron` format — nodes, roads (`Straight`, `Arc`, `Curve`, `Polyline`), lanes per
  direction, widths, speed limits, sidewalks, drive side; validation codes WP-N001..N007.
- `geometry.rs`: `Polyline` (sample, offset, slice/trim, project, resample) and shapes.
- `network.rs`: `RoadNetwork::build` trims roads at junctions, makes lanes (forward lanes on the
  right for right-hand traffic), adds junction connectors by turn rules (left turns from the inside
  lane, right turns from the outside, every lane continues through a two-road bend; no U-turns),
  `lane_at`, and A* `route` on travel time.
- `mesh.rs`: asphalt, junction fills, markings (dashed/solid/double centre), sidewalks with curbs.

Scenes reference a road file (`roads: Some("roads/town.road.ron")`); `wp_world::apply_scene` builds
the network into the `Roads` resource; the renderer caches its meshes per network.
Not yet: lane changes in routing, lane-drop tapers, elevation, traffic signals.
