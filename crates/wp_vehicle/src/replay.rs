//! Replays: a recorded input stream plus a hash of the world every N ticks.
//!
//! The simulation is deterministic, so the same build, setup, seed and
//! inputs give the same hashes. A golden replay catches any change in
//! behaviour, and the first checkpoint that differs shows when it began.
//! Files are `.replay.ron` (`wp schema replay`).

use crate::{Vehicle, VehicleInput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_ecs::{Entity, World};
use wp_physics::{Pose, Velocity};

/// A recorded session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Replay {
    /// What was driven, as given to `wp run`.
    pub setup: ReplaySetup,
    /// Ticks between world hashes.
    pub hash_every: u64,
    /// Ticks recorded (60 per simulated second).
    pub ticks: u64,
    /// The player's controls, each from its tick until the next change.
    #[serde(default)]
    pub inputs: Vec<InputChange>,
    /// Ticks before which the player was put back at the start (the R key).
    #[serde(default)]
    pub resets: Vec<u64>,
    /// World hash after every `hash_every` ticks.
    #[serde(default)]
    pub hashes: Vec<Checkpoint>,
    /// World hash after the last tick.
    pub final_hash: String,
}

/// The options a session was started with (see `wp run --help`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaySetup {
    /// Scenario name; its driver drives again during playback.
    #[serde(default)]
    pub scenario: Option<String>,
    #[serde(default)]
    pub scene: Option<String>,
    #[serde(default)]
    pub spawn: Option<String>,
    #[serde(default)]
    pub vehicle: Option<String>,
    #[serde(default)]
    pub trailer: Option<String>,
    #[serde(default)]
    pub mission: Option<String>,
    /// Quality tier the session ran at (it scales traffic): low, mid or flagship.
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputChange {
    pub tick: u64,
    pub throttle: f32,
    pub brake: f32,
    pub steer: f32,
    #[serde(default)]
    pub reverse: bool,
}

impl InputChange {
    fn input(&self) -> VehicleInput {
        VehicleInput {
            throttle: self.throttle,
            brake: self.brake,
            steer: self.steer,
            reverse: self.reverse,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub tick: u64,
    pub hash: String,
}

/// Where the player starts, for resets during recording and playback.
#[derive(Clone, Debug)]
pub struct StartState {
    pose: Pose,
    vehicle: Vehicle,
}

impl StartState {
    pub fn capture(world: &World, player: Entity) -> Option<StartState> {
        Some(StartState {
            pose: *world.get::<Pose>(player)?,
            vehicle: world.get::<Vehicle>(player)?.clone(),
        })
    }

    /// Put the player back at the start, at rest, with controls released.
    pub fn apply(&self, world: &mut World, player: Entity) {
        world.insert(player, self.pose);
        world.insert(player, Velocity::default());
        world.insert(player, self.vehicle.clone());
        world.insert(player, VehicleInput::default());
    }
}

/// Records a session tick by tick.
#[derive(Clone, Debug)]
pub struct Recorder {
    replay: Replay,
    last: Option<VehicleInput>,
}

impl Recorder {
    pub fn new(setup: ReplaySetup, hash_every: u64) -> Recorder {
        Recorder {
            replay: Replay {
                setup,
                hash_every: hash_every.max(1),
                ticks: 0,
                inputs: Vec::new(),
                resets: Vec::new(),
                hashes: Vec::new(),
                final_hash: String::new(),
            },
            last: None,
        }
    }

    /// The player's controls for the coming tick.
    pub fn before_tick(&mut self, input: VehicleInput) {
        if self.last != Some(input) {
            self.replay.inputs.push(InputChange {
                tick: self.replay.ticks,
                throttle: input.throttle,
                brake: input.brake,
                steer: input.steer,
                reverse: input.reverse,
            });
            self.last = Some(input);
        }
    }

    /// The player was reset to the start before the coming tick.
    pub fn reset(&mut self) {
        self.replay.resets.push(self.replay.ticks);
        self.last = Some(VehicleInput::default());
    }

    pub fn after_tick(&mut self, world: &World) {
        self.replay.ticks += 1;
        if self.replay.ticks.is_multiple_of(self.replay.hash_every) {
            self.replay.hashes.push(Checkpoint {
                tick: self.replay.ticks,
                hash: world.state_hash(),
            });
        }
    }

    pub fn ticks(&self) -> u64 {
        self.replay.ticks
    }

    pub fn finish(mut self, world: &World) -> Replay {
        self.replay.final_hash = world.state_hash();
        self.replay
    }
}

/// Where a playback first differed from the recording.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Divergence {
    /// The first checkpoint that differs; the change happened after
    /// `last_good_tick` (0: before the first checkpoint).
    pub tick: u64,
    pub last_good_tick: u64,
    pub expected: String,
    pub got: String,
}

/// Plays a replay back: inputs and resets before each tick, hash checks after.
#[derive(Clone, Debug)]
pub struct Playback {
    replay: Replay,
    tick: u64,
    next_input: usize,
    next_reset: usize,
    next_hash: usize,
    last_good: u64,
}

impl Playback {
    pub fn new(replay: Replay) -> Playback {
        Playback {
            replay,
            tick: 0,
            next_input: 0,
            next_reset: 0,
            next_hash: 0,
            last_good: 0,
        }
    }

    pub fn replay(&self) -> &Replay {
        &self.replay
    }

    pub fn done(&self) -> bool {
        self.tick >= self.replay.ticks
    }

    /// Apply this tick's reset and input change to the player, if any.
    pub fn before_tick(&mut self, world: &mut World, player: Entity, start: &StartState) {
        while self.replay.resets.get(self.next_reset) == Some(&self.tick) {
            start.apply(world, player);
            self.next_reset += 1;
        }
        while let Some(c) = self.replay.inputs.get(self.next_input)
            && c.tick <= self.tick
        {
            if let Some(mut i) = world.get_mut::<VehicleInput>(player) {
                *i = c.input();
            }
            self.next_input += 1;
        }
    }

    /// Check the world after a tick; returns the first divergence.
    pub fn after_tick(&mut self, world: &World) -> Option<Divergence> {
        self.tick += 1;
        let check = |expected: &str, tick: u64, last_good: &mut u64| {
            let got = world.state_hash();
            if got == expected {
                *last_good = tick;
                None
            } else {
                Some(Divergence {
                    tick,
                    last_good_tick: *last_good,
                    expected: expected.to_string(),
                    got,
                })
            }
        };
        if let Some(c) = self.replay.hashes.get(self.next_hash)
            && c.tick == self.tick
        {
            self.next_hash += 1;
            let (hash, tick) = (c.hash.clone(), c.tick);
            if let Some(d) = check(&hash, tick, &mut self.last_good) {
                return Some(d);
            }
        }
        if self.tick == self.replay.ticks {
            let hash = self.replay.final_hash.clone();
            return check(&hash, self.tick, &mut self.last_good);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_changes_only_and_plays_back() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, VehicleInput::default());
        let mut r = Recorder::new(ReplaySetup::default(), 2);
        for k in 0..6u64 {
            let input = VehicleInput {
                throttle: if k >= 3 { 1.0 } else { 0.0 },
                ..Default::default()
            };
            w.insert(e, input);
            r.before_tick(input);
            r.after_tick(&w);
        }
        let replay = r.finish(&w);
        assert_eq!(replay.ticks, 6);
        assert_eq!(replay.inputs.len(), 2, "{:?}", replay.inputs);
        assert_eq!(replay.inputs[1].tick, 3);
        assert_eq!(replay.hashes.len(), 3);

        // Same world: no divergence; a changed world: caught at the next check.
        let def: crate::VehicleDef = wp_data::parse_ron(
            "hatchback.vehicle.ron",
            include_str!("../../../examples/sandbox/vehicles/hatchback.vehicle.ron"),
        )
        .unwrap();
        let start = StartState {
            pose: Pose::default(),
            vehicle: Vehicle::new(&def),
        };
        w.insert(e, VehicleInput::default());
        let mut p = Playback::new(replay.clone());
        while !p.done() {
            p.before_tick(&mut w, e, &start);
            assert_eq!(p.after_tick(&w), None);
        }
        assert_eq!(w.get::<VehicleInput>(e).unwrap().throttle, 1.0);
        w.insert(e, VehicleInput::default());
        let mut p = Playback::new(replay);
        p.before_tick(&mut w, e, &start);
        assert_eq!(p.after_tick(&w), None);
        w.insert(e, Pose::default());
        p.before_tick(&mut w, e, &start);
        let d = p.after_tick(&w).expect("diverged");
        assert_eq!((d.tick, d.last_good_tick), (2, 0));
    }
}
