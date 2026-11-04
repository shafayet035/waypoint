//! Playing the player vehicle's sound on the default output device.

use std::sync::{Arc, Mutex};
use wp_audio::{Mixer, SoundInput, Source};
use wp_core::glam::Vec2;
use wp_ecs::{Entity, World};

/// What the vehicle `e` should sound like now (engine, road, skid).
pub fn sound_input(world: &World, e: Entity) -> Option<SoundInput> {
    let tel = wp_vehicle::telemetry(world, e)?;
    let veh = world.get::<wp_vehicle::Vehicle>(e)?;
    let input = world
        .get::<wp_vehicle::VehicleInput>(e)
        .map(|i| *i)
        .unwrap_or_default();
    let lib = world.get_resource::<wp_vehicle::VehicleLibrary>();
    let def = lib.as_ref().and_then(|l| l.0.get(&veh.model));
    let cylinders = def.map_or(4, |d| d.engine.cylinders);
    let radius = def.map_or(0.3, |d| d.wheel.radius_m);
    let sound = def.map(|d| d.sound.clone()).unwrap_or_default();
    let speed = tel.speed_kmh / 3.6;
    // Sliding sideways, or wheels spinning / locked against the road.
    let wheel_slip = veh
        .wheels
        .iter()
        .map(|w| (w.omega * radius - tel.forward_kmh / 3.6).abs() / speed.max(3.0))
        .fold(0.0f32, f32::max);
    let skid = (((tel.sideslip_deg.abs() - 6.0) / 10.0).clamp(0.0, 1.0)
        + ((wheel_slip - 0.2) * 2.0).clamp(0.0, 1.0))
    .min(1.0)
        * (speed / 4.0).clamp(0.0, 1.0);
    Some(SoundInput {
        rpm: tel.engine_rpm,
        throttle: input.throttle,
        cylinders,
        speed_mps: speed,
        skid,
        volume: 0.8 * sound.volume,
        tone: wp_audio::Tone {
            pitch: sound.pitch,
            roughness: sound.roughness,
            rumble: sound.rumble,
            intake: sound.intake,
            road: sound.road,
            skid: sound.skid,
        },
    })
}

/// Traffic heard at once, nearest first, and how far away it fades out.
const TRAFFIC_VOICES: usize = 5;
const HEARING_M: f32 = 60.0;

/// What the listener in vehicle `listener` hears: its own vehicle, and the
/// nearest traffic quieter with distance and panned by direction.
pub fn sound_sources(world: &World, listener: Entity) -> Vec<Source> {
    let mut out = Vec::new();
    let Some(me) = world.get::<wp_physics::Pose>(listener).map(|p| *p) else {
        return out;
    };
    if let Some(own) = sound_input(world, listener) {
        out.push(Source {
            key: key(listener),
            input: own,
            pan: 0.0,
        });
    }
    let right = Vec2::new(me.forward().y, -me.forward().x);
    let mut near: Vec<(f32, Entity, Vec2)> = Vec::new();
    world
        .query::<(&wp_physics::Pose, &wp_vehicle::Vehicle)>()
        .for_each(|e, (pose, _)| {
            let d = pose.position.distance(me.position);
            if e != listener && d < HEARING_M {
                near.push((d, e, pose.position - me.position));
            }
        });
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    for (d, e, rel) in near.into_iter().take(TRAFFIC_VOICES) {
        if let Some(mut input) = sound_input(world, e) {
            let fade = 1.0 - (d / HEARING_M).clamp(0.0, 1.0);
            input.volume *= 0.7 * fade * fade / (1.0 + d / 8.0);
            out.push(Source {
                key: key(e),
                input,
                pan: rel.normalize_or_zero().dot(right),
            });
        }
    }
    out
}

/// Plays the scene's vehicles on the default output device. The device opens on its
/// own thread (that can take seconds), so the window never waits for it;
/// without a usable device the game simply runs silent.
pub struct EngineAudio {
    input: Arc<Mutex<Vec<Source>>>,
    stop: std::sync::mpsc::Sender<()>,
}

impl Drop for EngineAudio {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

impl EngineAudio {
    pub fn start() -> EngineAudio {
        let input = Arc::new(Mutex::new(Vec::new()));
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let shared = input.clone();
        std::thread::spawn(move || {
            // The stream lives (and plays) until the window drops us.
            if let Some(_stream) = open_stream(shared) {
                let _ = stopped.recv();
            }
        });
        EngineAudio { input, stop }
    }

    pub fn set(&self, sources: Vec<Source>) {
        if let Ok(mut i) = self.input.lock() {
            *i = sources;
        }
    }
}

fn open_stream(input: Arc<Mutex<Vec<Source>>>) -> Option<cpal::Stream> {
    {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host().default_output_device()?;
        let config = device.default_output_config().ok()?;
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();
        let channels = config.channels as usize;
        let shared = input;
        let mut mixer = Mixer::new(config.sample_rate);
        let (mut left, mut right) = (Vec::new(), Vec::new());
        // Stereo frames (a mono device gets their average).
        let mut fill = move |frames: usize| -> Vec<(f32, f32)> {
            let now = shared.lock().map(|i| i.clone()).unwrap_or_default();
            left.resize(frames, 0.0);
            right.resize(frames, 0.0);
            mixer.render(&now, &mut left, &mut right);
            left.iter().copied().zip(right.iter().copied()).collect()
        };
        let err = |e: cpal::Error| eprintln!("audio: {e}");
        let stream = match format {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let m = fill(data.len() / channels);
                        for (frame, (l, r)) in data.chunks_mut(channels).zip(m) {
                            put(frame, l, r);
                        }
                    },
                    err,
                    None,
                )
                .ok()?,
            cpal::SampleFormat::I16 => device
                .build_output_stream(
                    config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        let m = fill(data.len() / channels);
                        for (frame, (l, r)) in data.chunks_mut(channels).zip(m) {
                            let mut f = [0.0f32; 8];
                            let n = frame.len().min(8);
                            put(&mut f[..n], l, r);
                            for (o, v) in frame.iter_mut().zip(f) {
                                *o = (v * i16::MAX as f32) as i16;
                            }
                        }
                    },
                    err,
                    None,
                )
                .ok()?,
            _ => return None,
        };
        stream.play().ok()?;
        Some(stream)
    }
}

/// Write a stereo frame into a device frame of any channel count.
fn put(frame: &mut [f32], l: f32, r: f32) {
    match frame.len() {
        0 => {}
        1 => frame[0] = (l + r) * 0.5,
        _ => {
            frame[0] = l;
            frame[1] = r;
            for x in &mut frame[2..] {
                *x = 0.0;
            }
        }
    }
}

fn key(e: Entity) -> u64 {
    ((e.index() as u64) << 32) | e.generation() as u64
}
