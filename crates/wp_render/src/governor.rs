//! Keeping frames on time: the render governor.
//!
//! Phones slow down when they get hot, and a scene can be heavier than the
//! benchmark that picked the tier. The governor watches frame times and the
//! OS thermal state and moves through [`STEPS`]: lower resolution first, then
//! cheaper effects (shadows, lights), then a 30 fps cap. It never touches
//! the simulation, which keeps running in whole ticks whatever the frame rate.
//!
//! Going down is quick (frames late for 1.5 s); coming back is a careful
//! probe: after a calm spell it tries the next better step, and if frames
//! slip again within a few seconds it returns and waits twice as long before
//! the next try. Heat sets a floor: Fair at least step 1, Serious step 3,
//! Critical step 5.

use serde::Serialize;

/// The OS's view of how hot the device is (iOS `thermalState`, Android
/// thermal status).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Thermal {
    #[default]
    Nominal,
    /// Warm: the OS has started to care.
    Fair,
    /// Hot: the OS will throttle soon or already does.
    Serious,
    /// Very hot: cut everything that can be cut.
    Critical,
}

impl Thermal {
    pub fn parse(s: &str) -> Option<Thermal> {
        match s {
            "nominal" => Some(Self::Nominal),
            "fair" => Some(Self::Fair),
            "serious" => Some(Self::Serious),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    /// The lowest step allowed at this temperature.
    pub fn floor(self) -> usize {
        match self {
            Self::Nominal => 0,
            Self::Fair => 1,
            Self::Serious => 3,
            Self::Critical => 5,
        }
    }
}

/// What one step of the governor renders, relative to the quality tier.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Step {
    /// Share of the tier's internal resolution (each side).
    pub render_scale: f32,
    /// Share of the tier's shadow-map size; 0: no shadow map (blob shadows).
    pub shadow_scale: f32,
    /// Share of the tier's local lights.
    pub light_scale: f32,
    /// Frames per second at most, if capped.
    pub fps_cap: Option<f32>,
}

const fn step(
    render_scale: f32,
    shadow_scale: f32,
    light_scale: f32,
    fps_cap: Option<f32>,
) -> Step {
    Step {
        render_scale,
        shadow_scale,
        light_scale,
        fps_cap,
    }
}

/// From full quality (0) to the cheapest (5): resolution, then effects,
/// then frame rate.
pub const STEPS: [Step; 6] = [
    step(1.0, 1.0, 1.0, None),
    step(0.85, 1.0, 1.0, None),
    step(0.7, 1.0, 1.0, None),
    step(0.7, 0.5, 0.5, None),
    step(0.6, 0.0, 0.25, None),
    step(0.6, 0.0, 0.25, Some(30.0)),
];

/// Seconds of late frames before stepping down.
const SLOW_S: f32 = 1.5;
/// A probe up that slips within this many seconds is undone.
const PROBE_S: f32 = 5.0;
/// Calm seconds before the first probe up; doubles after each failed one.
const FIRST_RETRY_S: f32 = 20.0;
const MAX_RETRY_S: f32 = 160.0;
/// Startup frames (shader compiles, uploads) are not judged.
const WARM_UP_S: f32 = 1.0;
/// After a change, frames are not judged while the new step settles in.
const SETTLE_S: f32 = 1.0;

/// Frame-time and heat control for one window.
#[derive(Clone, Debug, Serialize)]
pub struct Governor {
    /// The frame rate the tier aims for.
    pub target_fps: f32,
    level: usize,
    thermal: Thermal,
    /// Smoothed frame time, ms.
    smoothed_ms: f32,
    elapsed_s: f32,
    slow_s: f32,
    calm_s: f32,
    retry_s: f32,
    /// Seconds since the last probe up, while it is on trial.
    probing: Option<f32>,
    /// Seconds left before frames count again after a change.
    settle_s: f32,
    /// How many times the step changed.
    pub changes: u32,
}

impl Governor {
    pub fn new(target_fps: f32) -> Self {
        Governor {
            target_fps,
            level: 0,
            thermal: Thermal::Nominal,
            smoothed_ms: 1000.0 / target_fps,
            elapsed_s: 0.0,
            slow_s: 0.0,
            calm_s: 0.0,
            retry_s: FIRST_RETRY_S,
            probing: None,
            settle_s: 0.0,
            changes: 0,
        }
    }

    pub fn level(&self) -> usize {
        self.level
    }

    pub fn step(&self) -> Step {
        STEPS[self.level]
    }

    pub fn thermal(&self) -> Thermal {
        self.thermal
    }

    pub fn smoothed_ms(&self) -> f32 {
        self.smoothed_ms
    }

    /// The frame time this step aims for, ms.
    fn budget_ms(&self) -> f32 {
        let fps = match self.step().fps_cap {
            Some(cap) => cap.min(self.target_fps),
            None => self.target_fps,
        };
        1000.0 / fps
    }

    fn set(&mut self, level: usize) {
        if level != self.level {
            self.level = level;
            self.changes += 1;
            self.slow_s = 0.0;
            self.calm_s = 0.0;
            self.settle_s = SETTLE_S;
        }
    }

    /// Record a frame that took `frame_ms` (start to start) at heat
    /// `thermal`. Returns the new step when it changes.
    pub fn update(&mut self, frame_ms: f32, thermal: Thermal) -> Option<usize> {
        let before = self.level;
        let dt = (frame_ms / 1000.0).clamp(0.0, 0.25);
        self.thermal = thermal;
        // Heat raises the floor at once.
        if self.level < thermal.floor() {
            self.probing = None;
            self.set(thermal.floor());
        }
        self.elapsed_s += dt;
        if self.elapsed_s < WARM_UP_S {
            return (self.level != before).then_some(self.level);
        }
        self.smoothed_ms += (frame_ms - self.smoothed_ms) * (1.0 - (-dt / 0.5).exp());
        if self.settle_s > 0.0 {
            self.settle_s -= dt;
            return (self.level != before).then_some(self.level);
        }
        let budget = self.budget_ms();
        if let Some(t) = &mut self.probing {
            *t += dt;
            if *t > PROBE_S {
                // The better step held: keep it and start afresh.
                self.probing = None;
                self.retry_s = FIRST_RETRY_S;
            }
        }
        if self.smoothed_ms > budget * 1.15 {
            self.slow_s += dt;
            self.calm_s = 0.0;
        } else {
            self.slow_s = (self.slow_s - dt * 0.5).max(0.0);
            if self.smoothed_ms <= budget * 1.05 {
                self.calm_s += dt;
            }
        }
        if self.slow_s > SLOW_S && self.level + 1 < STEPS.len() {
            if self.probing.take().is_some() {
                self.retry_s = (self.retry_s * 2.0).min(MAX_RETRY_S);
            }
            self.set(self.level + 1);
        } else if self.calm_s > self.retry_s
            && self.level > thermal.floor()
            && self.probing.is_none()
        {
            self.probing = Some(0.0);
            self.set(self.level - 1);
        }
        (self.level != before).then_some(self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `seconds` of frames taking `ms` each; the levels seen.
    fn run(g: &mut Governor, ms: f32, seconds: f32, heat: Thermal) -> Vec<usize> {
        let mut seen = Vec::new();
        let mut t = 0.0;
        while t < seconds {
            if let Some(l) = g.update(ms, heat) {
                seen.push(l);
            }
            t += ms / 1000.0;
        }
        seen
    }

    #[test]
    fn on_time_frames_change_nothing() {
        let mut g = Governor::new(60.0);
        assert!(run(&mut g, 16.7, 120.0, Thermal::Nominal).is_empty());
        assert_eq!(g.level(), 0);
    }

    #[test]
    fn slow_frames_step_down_resolution_first() {
        let mut g = Governor::new(60.0);
        let seen = run(&mut g, 25.0, 4.0, Thermal::Nominal);
        assert_eq!(seen.first(), Some(&1), "{seen:?}");
        assert!(g.step().render_scale < 1.0 && g.step().shadow_scale == 1.0);
        // Still slow: on down to the frame cap, never past the last step.
        run(&mut g, 40.0, 60.0, Thermal::Nominal);
        assert_eq!(g.level(), STEPS.len() - 1);
        assert_eq!(g.step().fps_cap, Some(30.0));
    }

    #[test]
    fn a_failed_probe_waits_twice_as_long() {
        let mut g = Governor::new(60.0);
        run(&mut g, 25.0, 4.0, Thermal::Nominal);
        assert_eq!(g.level(), 1);
        // Calm at step 1 for 20 s: it tries step 0 again...
        let seen = run(&mut g, 16.7, 22.0, Thermal::Nominal);
        assert_eq!(seen, vec![0]);
        // ...which is too slow, so it goes back to 1.
        let seen = run(&mut g, 25.0, 3.0, Thermal::Nominal);
        assert_eq!(seen, vec![1]);
        // The next try waits 40 s, not 20.
        assert!(run(&mut g, 16.7, 30.0, Thermal::Nominal).is_empty());
        assert_eq!(run(&mut g, 16.7, 15.0, Thermal::Nominal), vec![0]);
        // That one holds.
        assert!(run(&mut g, 16.7, 30.0, Thermal::Nominal).is_empty());
        assert_eq!(g.level(), 0);
    }

    #[test]
    fn heat_sets_a_floor_and_lifts_slowly() {
        let mut g = Governor::new(60.0);
        assert_eq!(g.update(16.7, Thermal::Serious), Some(3));
        assert_eq!(g.step().shadow_scale, 0.5);
        assert_eq!(g.update(16.7, Thermal::Critical), Some(5));
        // Cooler again: one step at a time, after calm spells.
        let seen = run(&mut g, 16.7, 90.0, Thermal::Nominal);
        // One step about every 20 s.
        assert_eq!(seen, vec![4, 3, 2, 1], "{seen:?}");
    }

    #[test]
    fn capped_steps_judge_against_their_cap() {
        let mut g = Governor::new(60.0);
        g.update(33.3, Thermal::Critical);
        // 30 fps is on time at the capped step.
        assert!(run(&mut g, 33.3, 60.0, Thermal::Critical).is_empty());
    }
}
