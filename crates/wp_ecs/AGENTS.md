# wp_ecs

Deterministic ECS.

- Components implement `Component` with a stable `NAME` and serde. Insert with `world.insert(e, c)`.
- Queries: `world.query::<(&mut Pose, &Velocity)>().for_each(|e, (p, v)| ...)`. Up to 6 parameters.
- Resources: `world.resource::<T>()` / `resource_mut`. Structural changes need `&mut World`.
- Schedule: stages Input → Agents → Physics (×substeps) → Post → RenderExtract. Within a stage,
  `.before()/.after()` constraints, then registration order. Declare `.reads::<T>()/.writes::<T>()`.
- Reflection: `entity_json`, `set_component_json`, `to_json`, `state_hash`.

Pitfalls: the same component twice in one query panics (`&mut A, &A`). Iteration order follows
insert/remove history, which is deterministic; do not sort by `TypeId`.
