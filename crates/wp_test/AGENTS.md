# wp_test

Scenario runner. `run_scenario(input, make_app)` spawns the player, drives it with a `Script` or
`Cruise` driver in the Agents stage, records per-tick samples, hashes the world every second and
evaluates assertions. `Deterministic` runs the scenario twice and reports the first second the
hashes differ. Add a new assertion by extending `Assertion`, `describe()`, `evaluate()` and
`wp docs scenario`.
