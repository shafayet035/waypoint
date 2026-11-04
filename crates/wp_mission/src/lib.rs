//! Missions: a bus route through the stops of a scene's road network, with
//! a timetable, passengers who board and alight, and a score for
//! punctuality, comfort and safety. A mission is a `missions/<name>.mission.ron`
//! file; the player (or an autopilot) drives it.
//!
//! The mission watches the player's vehicle each tick: stopping with the bus
//! at a stop (within 5 m along and 1.5 m across, below 0.5 km/h) opens the
//! doors; people get off and on (seconds per person); then the bus may go.
//! Leaving early leaves people behind. After the last stop the mission is
//! complete.

use glam::Vec2;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use wp_app::{App, Plugin};
use wp_core::{SimRng, SimTime};
use wp_ecs::{Entity, Events, Stage, World};
use wp_physics::{Pose, Velocity};
use wp_road::RoadNetwork;
use wp_vehicle::VehicleEvent;
use wp_world::{Player, Roads};

/// A mission file: `missions/<name>.mission.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MissionDef {
    pub title: String,
    /// Shown before the start.
    #[serde(default)]
    pub briefing: String,
    /// Scene path (`scenes/<name>.scene.ron`); its road network has the stops.
    pub scene: String,
    /// Vehicle name.
    pub vehicle: String,
    /// Spawn point in the scene.
    pub spawn: String,
    /// The stops to serve, in order, with the timetable.
    pub stops: Vec<MissionStop>,
    #[serde(default)]
    pub passengers: PassengerDef,
    /// Fail if the route is not finished within this many seconds.
    #[serde(default)]
    pub time_limit_s: Option<f32>,
    /// Lua script with mission hooks (`missions/<name>.lua`), if any.
    #[serde(default)]
    pub script: Option<String>,
}

/// One stop on the route.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MissionStop {
    /// Stop id in the road network.
    pub stop: String,
    /// Timetabled departure, s after the start.
    pub depart_s: f32,
}

/// How many people use the stops and how long they take.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PassengerDef {
    /// People waiting at each stop: a random number in [min, max].
    #[serde(default = "default_waiting_min")]
    pub waiting_min: u32,
    #[serde(default = "default_waiting_max")]
    pub waiting_max: u32,
    /// Share of the people on board who get off at each stop.
    #[serde(default = "default_alighting")]
    pub alighting_share: f32,
    /// Seconds per person getting on (paying) and off.
    #[serde(default = "default_board_s")]
    pub board_s_each: f32,
    #[serde(default = "default_alight_s")]
    pub alight_s_each: f32,
    /// Seats and standing room.
    #[serde(default = "default_capacity")]
    pub capacity: u32,
}

impl Default for PassengerDef {
    fn default() -> Self {
        Self {
            waiting_min: default_waiting_min(),
            waiting_max: default_waiting_max(),
            alighting_share: default_alighting(),
            board_s_each: default_board_s(),
            alight_s_each: default_alight_s(),
            capacity: default_capacity(),
        }
    }
}

fn default_waiting_min() -> u32 {
    1
}
fn default_waiting_max() -> u32 {
    6
}
fn default_alighting() -> f32 {
    0.3
}
fn default_board_s() -> f32 {
    2.5
}
fn default_alight_s() -> f32 {
    1.5
}
fn default_capacity() -> u32 {
    70
}

/// Something that happened in the mission (`events.poll` shows them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum MissionEvent {
    /// The doors opened at a stop.
    Arrived {
        stop: String,
        t: f32,
    },
    /// The bus left a stop; `late_s` against the timetable (negative: early).
    Departed {
        stop: String,
        t: f32,
        late_s: f32,
        boarded: u32,
        alighted: u32,
        left_behind: u32,
    },
    Completed {
        t: f32,
        score: u32,
    },
    Failed {
        t: f32,
        reason: String,
    },
}

/// Where the mission stands (`world.resources` shows it as "Mission").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MissionStatus {
    pub title: String,
    /// What to do now, in words.
    pub objective: String,
    /// Index into the route of the stop being driven to (or served).
    pub next_stop: usize,
    pub stops: usize,
    pub on_board: u32,
    /// Straight-line distance to the next stop, m.
    pub distance_m: f32,
    pub doors_open: bool,
    /// The last message from the mission script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Lines the script logged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<String>,
    pub completed: bool,
    pub failed: bool,
    pub score: Score,
}

/// Points out of 100 each, and the overall score (their mean).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub punctuality: u32,
    pub comfort: u32,
    pub safety: u32,
    pub total: u32,
}

impl Default for Score {
    fn default() -> Self {
        Self {
            punctuality: 100,
            comfort: 100,
            safety: 100,
            total: 100,
        }
    }
}

/// A stop the mission uses, resolved in the road network.
#[derive(Clone, Debug)]
pub struct RouteStop {
    pub id: String,
    pub name: String,
    pub position: Vec2,
    pub direction: Vec2,
    pub depart_s: f32,
    pub waiting: u32,
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Driving,
    /// Doors open: seconds of boarding left, and who gets on and off here.
    Boarding {
        left_s: f32,
        boarding: u32,
        alighting: u32,
    },
    Done,
}

/// The running mission.
pub struct MissionState {
    pub def: MissionDef,
    pub route: Vec<RouteStop>,
    phase: Phase,
    next: usize,
    on_board: u32,
    /// Lateness at each served stop, s.
    lateness: Vec<f32>,
    left_behind: u32,
    /// Moments of harsh driving (sideways or braking/accelerating over 0.3 g).
    harsh: u32,
    harsh_now: bool,
    collisions: u32,
    red_lights: u32,
    people_hit: u32,
    failed: Option<String>,
    /// The mission's Lua script, if any, and what it has said.
    script: Option<wp_script::Script>,
    started: bool,
    objective_override: Option<String>,
    message: Option<String>,
    bonus: i32,
    log: Vec<String>,
}

impl MissionState {
    pub fn next_stop(&self) -> usize {
        self.next
    }
    /// The route stop the doors are open at, if boarding.
    pub fn boarding_at(&self) -> Option<usize> {
        matches!(self.phase, Phase::Boarding { .. }).then_some(self.next)
    }
    /// Whether the bus has served `stop` (left it).
    pub fn served(&self, stop: usize) -> bool {
        stop < self.next || self.phase == Phase::Done
    }
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done || self.failed.is_some()
    }

    fn score(&self) -> Score {
        let punctuality = if self.lateness.is_empty() {
            100.0
        } else {
            // Full marks from 30 s early to 60 s late, falling to 0 at 5 minutes late.
            let per: f32 = self
                .lateness
                .iter()
                .map(|&l| {
                    if (-30.0..=60.0).contains(&l) {
                        100.0
                    } else if l < -30.0 {
                        (100.0 + (l + 30.0) * 0.5).max(0.0)
                    } else {
                        (100.0 - (l - 60.0) / 2.4).max(0.0)
                    }
                })
                .sum::<f32>()
                / self.lateness.len() as f32;
            (per - self.left_behind as f32 * 5.0).max(0.0)
        };
        let comfort = (100.0 - self.harsh as f32 * 4.0).max(0.0);
        let safety = (100.0
            - self.collisions as f32 * 25.0
            - self.red_lights as f32 * 15.0
            - self.people_hit as f32 * 50.0)
            .max(0.0);
        let total = ((punctuality + comfort + safety) / 3.0 + self.bonus as f32).clamp(0.0, 100.0);
        Score {
            punctuality: punctuality.round() as u32,
            comfort: comfort.round() as u32,
            safety: safety.round() as u32,
            total: total.round() as u32,
        }
    }
}

/// Resolve a mission's stops in a road network. Errors name the missing stop.
pub fn resolve_route(
    def: &MissionDef,
    net: &RoadNetwork,
    seed: u64,
) -> Result<Vec<RouteStop>, String> {
    let mut rng = SimRng::fork(seed, "passengers");
    def.stops
        .iter()
        .map(|ms| {
            let st = net.stops.iter().find(|s| s.id == ms.stop).ok_or_else(|| {
                format!("stop `{}` is not in road network `{}`", ms.stop, net.name)
            })?;
            let span = def
                .passengers
                .waiting_max
                .saturating_sub(def.passengers.waiting_min)
                + 1;
            Ok(RouteStop {
                id: st.id.clone(),
                name: st.name.clone(),
                position: st.position,
                direction: st.direction,
                depart_s: ms.depart_s,
                waiting: def.passengers.waiting_min + (rng.next_u64() % span as u64) as u32,
            })
        })
        .collect()
}

/// Start `def` in a world whose scene (with roads) is set up and whose
/// player exists. Errors if a stop is missing.
/// `script` is the mission script's (file name, source), if it has one.
pub fn start(
    world: &mut World,
    def: &MissionDef,
    script: Option<(&str, &str)>,
) -> Result<(), String> {
    let net = world
        .get_resource::<Roads>()
        .map(|r| r.0.clone())
        .ok_or("the mission's scene has no road network")?;
    let seed = world
        .get_resource::<SimRng>()
        .map_or(0, |r| r.clone().next_u64());
    let route = resolve_route(def, &net, seed)?;
    let script = match script {
        Some((name, source)) => Some(
            wp_script::Script::load(source, name, seed)
                .map_err(|e| format!("script {name}: {e}"))?,
        ),
        None => None,
    };
    world.insert_resource(MissionState {
        script,
        started: false,
        objective_override: None,
        message: None,
        bonus: 0,
        log: Vec::new(),
        def: def.clone(),
        route,
        phase: Phase::Driving,
        next: 0,
        on_board: 0,
        lateness: Vec::new(),
        left_behind: 0,
        harsh: 0,
        harsh_now: false,
        collisions: 0,
        red_lights: 0,
        people_hit: 0,
        failed: None,
    });
    Ok(())
}

pub struct MissionPlugin;

impl Plugin for MissionPlugin {
    fn name(&self) -> &'static str {
        "mission"
    }
    fn dependencies(&self) -> Vec<&'static str> {
        vec!["vehicle", "world"]
    }
    fn build(&self, app: &mut App) {
        app.add_event::<MissionEvent>("MissionEvent");
        app.add_system(Stage::Post, "mission", mission)
            .reads::<Pose>()
            .reads::<Velocity>()
            .reads_resource("Events<VehicleEvent>")
            .writes_resource("MissionState")
            .writes_resource("Events<MissionEvent>");
    }
}

/// Distance along and across a stop's lane from where a bus should stand.
fn offset_from(stop: &RouteStop, p: Vec2) -> (f32, f32) {
    let d = p - stop.position;
    (d.dot(stop.direction), d.perp_dot(stop.direction).abs())
}

fn mission(world: &mut World) {
    if !world.has_resource::<MissionState>() {
        return;
    }
    let Some(player) = world.query::<&Player>().entities().first().copied() else {
        return;
    };
    let t = world.resource::<SimTime>().seconds() as f32;
    let dt = world.resource::<SimTime>().dt();
    let (Some(pose), Some(vel)) = (
        world.get::<Pose>(player).map(|p| *p),
        world.get::<Velocity>(player).map(|v| *v),
    ) else {
        return;
    };
    let tel = wp_vehicle::telemetry(world, player);
    let me = player.to_string();
    let (mut hits, mut reds, mut people) = (0, 0, 0);
    if let Some(events) = world.get_resource::<Events<VehicleEvent>>() {
        for e in events.iter().filter(|e| e.entity() == me) {
            match e.kind() {
                "Collision" => hits += 1,
                "RanRedLight" => reds += 1,
                "HitPedestrian" => people += 1,
                _ => {}
            }
        }
    }
    let mut out: Vec<MissionEvent> = Vec::new();
    let status = {
        let mut st = world.resource_mut::<MissionState>();
        st.collisions += hits;
        st.red_lights += reds;
        st.people_hit += people;
        let speed_kmh = vel.linear.length() * 3.6;
        if let Some(tel) = &tel {
            // One harsh moment per manoeuvre: over 0.3 g, counted again only
            // after settling below 0.2 g.
            let g = tel.lateral_g.abs().max(tel.longitudinal_g.abs());
            if g > 0.3 && !st.harsh_now && st.phase == Phase::Driving {
                st.harsh += 1;
                st.harsh_now = true;
                let what = if tel.lateral_g.abs() > tel.longitudinal_g.abs() {
                    "cornering"
                } else if tel.longitudinal_g < 0.0 {
                    "braking"
                } else {
                    "accelerating"
                };
                let at = tel.position;
                st.log.push(format!(
                    "{t:.1}s harsh {what} {g:.2} g at ({:.0}, {:.0}), {speed_kmh:.0} km/h",
                    at[0], at[1]
                ));
            } else if g < 0.2 {
                st.harsh_now = false;
            }
        }
        if let Some(limit) = st.def.time_limit_s
            && t > limit
            && st.phase != Phase::Done
            && st.failed.is_none()
        {
            st.failed = Some(format!("out of time ({limit:.0} s)"));
            out.push(MissionEvent::Failed {
                t,
                reason: format!("out of time ({limit:.0} s)"),
            });
        }
        match st.phase.clone() {
            Phase::Driving if st.failed.is_none() => {
                let stop = st.route[st.next].clone();
                let (along, across) = offset_from(&stop, pose.position);
                if along.abs() < 5.0 && across < 1.5 && speed_kmh < 0.5 {
                    let alighting =
                        (st.on_board as f32 * st.def.passengers.alighting_share).round() as u32;
                    let room = st.def.passengers.capacity - (st.on_board - alighting);
                    let boarding = stop.waiting.min(room);
                    let left_s = 3.0
                        + alighting as f32 * st.def.passengers.alight_s_each
                        + boarding as f32 * st.def.passengers.board_s_each;
                    st.phase = Phase::Boarding {
                        left_s,
                        boarding,
                        alighting,
                    };
                    out.push(MissionEvent::Arrived {
                        stop: stop.id.clone(),
                        t,
                    });
                }
            }
            Phase::Boarding {
                left_s,
                boarding,
                alighting,
            } => {
                let stop = st.route[st.next].clone();
                let total = 3.0
                    + alighting as f32 * st.def.passengers.alight_s_each
                    + boarding as f32 * st.def.passengers.board_s_each;
                let moved = speed_kmh > 1.0;
                let left = left_s - dt;
                if left <= 0.0 || moved {
                    // Who got on before the doors closed.
                    let done = ((total - left.max(0.0)) / total).clamp(0.0, 1.0);
                    let got_on = if moved {
                        (boarding as f32 * done).floor() as u32
                    } else {
                        boarding
                    };
                    let behind = boarding - got_on;
                    st.on_board = st.on_board - alighting + got_on;
                    st.left_behind += behind;
                    let late = t - stop.depart_s;
                    st.lateness.push(late);
                    out.push(MissionEvent::Departed {
                        stop: stop.id.clone(),
                        t,
                        late_s: late,
                        boarded: got_on,
                        alighted: alighting,
                        left_behind: behind,
                    });
                    st.next += 1;
                    if st.next >= st.route.len() {
                        st.phase = Phase::Done;
                        let score = st.score().total;
                        out.push(MissionEvent::Completed { t, score });
                    } else {
                        st.phase = Phase::Driving;
                    }
                } else {
                    st.phase = Phase::Boarding {
                        left_s: left,
                        boarding,
                        alighting,
                    };
                }
            }
            _ => {}
        }
        // The script: hooks for this tick's events, then its commands.
        if st.script.is_some() {
            let view = wp_script::View {
                time_s: t,
                speed_kmh,
                on_board: st.on_board,
                next_stop: st.route[st.next.min(st.route.len() - 1)].id.clone(),
                stops_served: st.next as u32,
            };
            let script = st.script.as_ref().expect("checked");
            script.set_view(view);
            let mut result = Ok(());
            if !st.started {
                result = result.and(script.call("on_start", ()));
            }
            result = result.and(script.call("on_tick", dt));
            for e in &out {
                result = result.and(match e {
                    MissionEvent::Arrived { stop, .. } => script.call("on_arrive", stop.clone()),
                    MissionEvent::Departed {
                        stop,
                        late_s,
                        boarded,
                        alighted,
                        left_behind,
                        ..
                    } => script.call_with_info(
                        "on_depart",
                        stop,
                        &[
                            ("late_s", *late_s as f64),
                            ("boarded", *boarded as f64),
                            ("alighted", *alighted as f64),
                            ("left_behind", *left_behind as f64),
                        ],
                    ),
                    MissionEvent::Completed { score, .. } => script.call("on_complete", *score),
                    MissionEvent::Failed { reason, .. } => script.call("on_fail", reason.clone()),
                });
            }
            let commands = script.take_commands();
            st.started = true;
            if let Err(e) = result
                && st.failed.is_none()
            {
                let reason = format!("script error: {e}");
                st.failed = Some(reason.clone());
                out.push(MissionEvent::Failed { t, reason });
            }
            for c in commands {
                match c {
                    wp_script::Command::Objective(text) => st.objective_override = Some(text),
                    wp_script::Command::Message(text) => st.message = Some(text),
                    wp_script::Command::AddPassengers { stop, count } => {
                        let from = st.next;
                        if let Some(rs) = st.route.iter_mut().skip(from).find(|r| r.id == stop) {
                            rs.waiting += count;
                        }
                    }
                    wp_script::Command::Bonus(points) => st.bonus += points,
                    wp_script::Command::Fail(reason) => {
                        if st.failed.is_none() {
                            st.failed = Some(reason.clone());
                            out.push(MissionEvent::Failed { t, reason });
                        }
                    }
                    wp_script::Command::Log(text) => {
                        st.log.push(format!("{t:.1}s {text}"));
                        if st.log.len() > 50 {
                            st.log.remove(0);
                        }
                    }
                }
            }
        }
        let next = st.next.min(st.route.len().saturating_sub(1));
        let stop = &st.route[next];
        MissionStatus {
            message: st.message.clone(),
            log: st.log.clone(),
            title: st.def.title.clone(),
            objective: match (&st.phase, &st.failed) {
                (_, Some(reason)) => format!("Mission failed: {reason}"),
                (Phase::Done, _) => format!("Route complete: score {}", st.score().total),
                (
                    Phase::Boarding {
                        boarding,
                        alighting,
                        ..
                    },
                    _,
                ) => format!(
                    "{}: {alighting} getting off, {boarding} getting on",
                    stop.name
                ),
                (Phase::Driving, _) if st.objective_override.is_some() => {
                    st.objective_override.clone().unwrap_or_default()
                }
                (Phase::Driving, _) => {
                    let due = stop.depart_s - t;
                    format!(
                        "Next stop: {} ({}/{}), due in {:.0} s",
                        stop.name,
                        next + 1,
                        st.route.len(),
                        due
                    )
                }
            },
            next_stop: next,
            stops: st.route.len(),
            on_board: st.on_board,
            distance_m: stop.position.distance(pose.position),
            doors_open: matches!(st.phase, Phase::Boarding { .. }),
            completed: st.phase == Phase::Done,
            failed: st.failed.is_some(),
            score: st.score(),
        }
    };
    world.insert_reflected_resource("Mission", status);
    if let Some(mut events) = world.get_resource_mut::<Events<MissionEvent>>() {
        for e in out {
            events.send(e);
        }
    }
}

/// The player entity, for drivers that act on the mission's vehicle.
pub fn player(world: &World) -> Option<Entity> {
    world.query::<&Player>().entities().first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(lateness: Vec<f32>, harsh: u32, collisions: u32) -> MissionState {
        MissionState {
            def: MissionDef {
                title: "t".into(),
                briefing: String::new(),
                scene: String::new(),
                vehicle: String::new(),
                spawn: String::new(),
                stops: vec![],
                passengers: PassengerDef::default(),
                time_limit_s: None,
                script: None,
            },
            route: vec![],
            phase: Phase::Driving,
            next: 0,
            on_board: 0,
            lateness,
            left_behind: 0,
            harsh,
            harsh_now: false,
            collisions,
            red_lights: 0,
            people_hit: 0,
            failed: None,
            script: None,
            started: false,
            objective_override: None,
            message: None,
            bonus: 0,
            log: Vec::new(),
        }
    }

    #[test]
    fn scores_punctuality_comfort_and_safety() {
        let perfect = state(vec![0.0, 30.0, -20.0], 0, 0).score();
        assert_eq!(
            (
                perfect.punctuality,
                perfect.comfort,
                perfect.safety,
                perfect.total
            ),
            (100, 100, 100, 100)
        );
        // Three minutes late at one of two stops: that stop scores 50.
        let late = state(vec![0.0, 180.0], 0, 0).score();
        assert_eq!(late.punctuality, 75);
        // Five harsh moments and a collision.
        let rough = state(vec![0.0], 5, 1).score();
        assert_eq!((rough.comfort, rough.safety), (80, 75));
    }
}
