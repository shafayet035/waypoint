//! Scenario tests: the engine's main verification tool.
//!
//! A `.scenario.ron` file names a scene, a vehicle, a spawn point, a driver
//! (scripted inputs or a cruise controller) and assertions. [`run_scenario`]
//! runs it headless at a fixed timestep and returns a [`ScenarioReport`] with
//! pass/fail per assertion, metrics and a state hash.

mod def;
mod runner;

pub use def::{Assertion, Driver, InputKey, PlayerDef, ScenarioDef};
pub use runner::{AssertionResult, Metrics, ScenarioInput, ScenarioReport, run_scenario};
