//! The fixed-timestep simulation clock.
//!
//! Gameplay and AI tick at `tick_hz` (60 Hz). Physics runs `substeps` times per
//! tick (8 → 480 Hz), so every physics rate divides evenly into a tick.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimTime {
    /// Completed gameplay ticks since the simulation started.
    pub tick: u64,
    pub tick_hz: u32,
    pub substeps: u32,
}

impl Default for SimTime {
    fn default() -> Self {
        Self {
            tick: 0,
            tick_hz: 60,
            substeps: 8,
        }
    }
}

impl SimTime {
    /// Gameplay timestep in seconds.
    pub fn dt(&self) -> f32 {
        1.0 / self.tick_hz as f32
    }
    /// Physics timestep in seconds.
    pub fn physics_dt(&self) -> f32 {
        1.0 / (self.tick_hz * self.substeps) as f32
    }
    pub fn physics_hz(&self) -> u32 {
        self.tick_hz * self.substeps
    }
    /// Simulated seconds elapsed, computed from the tick count so it never drifts.
    pub fn seconds(&self) -> f64 {
        self.tick as f64 / self.tick_hz as f64
    }
    /// Number of ticks covering `seconds` (rounded up).
    pub fn ticks_for(&self, seconds: f32) -> u64 {
        (seconds as f64 * self.tick_hz as f64).ceil().max(0.0) as u64
    }
}
