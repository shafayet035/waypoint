//! Playing the player vehicle's sound on the default output device.

use std::sync::{Arc, Mutex};
use wp_audio::{SoundInput, Voice};
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

/// Plays one voice on the default output device. The device opens on its
/// own thread (that can take seconds), so the window never waits for it;
/// without a usable device the game simply runs silent.
pub struct EngineAudio {
    input: Arc<Mutex<SoundInput>>,
    stop: std::sync::mpsc::Sender<()>,
}

impl Drop for EngineAudio {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

impl EngineAudio {
    pub fn start() -> EngineAudio {
        let input = Arc::new(Mutex::new(SoundInput::default()));
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

    pub fn set(&self, input: SoundInput) {
        if let Ok(mut i) = self.input.lock() {
            *i = input;
        }
    }
}

fn open_stream(input: Arc<Mutex<SoundInput>>) -> Option<cpal::Stream> {
    {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host().default_output_device()?;
        let config = device.default_output_config().ok()?;
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();
        let channels = config.channels as usize;
        let shared = input;
        let mut voice = Voice::new(config.sample_rate);
        let mut mono = Vec::new();
        let mut fill = move |frames: usize| -> Vec<f32> {
            let now = shared.lock().map(|i| *i).unwrap_or_default();
            mono.resize(frames, 0.0);
            voice.render(&now, &mut mono);
            mono.clone()
        };
        let err = |e: cpal::Error| eprintln!("audio: {e}");
        let stream = match format {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        let m = fill(data.len() / channels);
                        for (frame, s) in data.chunks_mut(channels).zip(m) {
                            frame.fill(s);
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
                        for (frame, s) in data.chunks_mut(channels).zip(m) {
                            frame.fill((s * i16::MAX as f32) as i16);
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
