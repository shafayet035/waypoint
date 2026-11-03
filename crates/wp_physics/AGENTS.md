# wp_physics

Planar rigid bodies (M0). x east, y north, heading counter-clockwise from +x; body frame x forward,
y left. Forces are applied with `RigidBody::apply_force` in Physics systems that run
`.before("integrate_bodies")`. Integration is semi-implicit Euler at 480 Hz.
