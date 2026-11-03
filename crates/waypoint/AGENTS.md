# waypoint

The facade. `DefaultPlugins` = physics, world, vehicle. `make_app(disabled)` builds an unfinished
app. `project::Project::load(root)` loads and cross-checks every project file and returns all
diagnostics at once. `tests/sandbox_scenarios.rs` is the M0 exit gate.
