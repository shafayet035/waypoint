//! Vehicle sound, synthesized: no recordings to license, and every vehicle
//! sounds like its own engine.
//!
//! An engine fires `cylinders / 2` times per revolution, so its note is
//! `rpm / 60 × cylinders / 2` Hz. The voice sums harmonics of that note
//! (louder high harmonics under load), a half-order rumble, and intake
//! noise that grows with throttle and rpm. Tyres add road roar with speed
//! and a squeal when they slide. [`Voice::render`] fills a buffer from the
//! latest [`SoundInput`], gliding from the previous one so changes never click.
//!
//! The output is plain `f32` samples: a window plays them through the sound
//! card, and `wp audio` writes them to a WAV file that agents can analyse.

/// What the listener should hear now (one vehicle).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundInput {
    pub rpm: f32,
    /// 0–1.
    pub throttle: f32,
    pub cylinders: u8,
    /// Road speed, m/s (tyre roar).
    pub speed_mps: f32,
    /// 0 (gripping) to 1 (sliding hard): squeal.
    pub skid: f32,
    /// Overall level, 0–1.
    pub volume: f32,
    /// The vehicle's character (multipliers, 1 = default).
    pub tone: Tone,
}

/// A vehicle's sound character; every field multiplies the default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub pitch: f32,
    pub roughness: f32,
    pub rumble: f32,
    pub intake: f32,
    pub road: f32,
    pub skid: f32,
}

impl Default for Tone {
    fn default() -> Self {
        Tone {
            pitch: 1.0,
            roughness: 1.0,
            rumble: 1.0,
            intake: 1.0,
            road: 1.0,
            skid: 1.0,
        }
    }
}

impl Default for SoundInput {
    fn default() -> Self {
        SoundInput {
            rpm: 0.0,
            throttle: 0.0,
            cylinders: 4,
            speed_mps: 0.0,
            skid: 0.0,
            volume: 0.0,
            tone: Tone::default(),
        }
    }
}

impl SoundInput {
    /// The engine's firing frequency, Hz.
    pub fn firing_hz(&self) -> f32 {
        self.rpm / 60.0 * self.cylinders.max(1) as f32 / 2.0
    }
}

/// Harmonic levels of the firing note at idle and at full load.
const HARMONICS: [(f32, f32); 6] = [
    (1.0, 1.0),
    (0.45, 0.7),
    (0.22, 0.5),
    (0.12, 0.38),
    (0.06, 0.25),
    (0.03, 0.16),
];

/// One vehicle's sound generator. Keeps its phases and filter state between
/// buffers, so the sound is continuous.
#[derive(Clone, Debug)]
pub struct Voice {
    sample_rate: f32,
    phase: f64,
    last: SoundInput,
    noise: u32,
    intake_lp: f32,
    roar_lp: f32,
    roar_lp2: f32,
    squeal_phase: f64,
}

impl Voice {
    pub fn new(sample_rate: u32) -> Voice {
        Voice {
            sample_rate: sample_rate as f32,
            phase: 0.0,
            last: SoundInput::default(),
            noise: 0x1234_5678,
            intake_lp: 0.0,
            roar_lp: 0.0,
            roar_lp2: 0.0,
            squeal_phase: 0.0,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate as u32
    }

    fn white(&mut self) -> f32 {
        // xorshift32: cheap, deterministic noise.
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Fill `out` (mono, −1 to 1) gliding from the last input to `input`.
    pub fn render(&mut self, input: &SoundInput, out: &mut [f32]) {
        let n = out.len().max(1) as f32;
        let from = self.last;
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let dt = 1.0 / self.sample_rate;
        for (k, s) in out.iter_mut().enumerate() {
            let t = (k as f32 + 1.0) / n;
            let rpm = lerp(from.rpm, input.rpm, t);
            let throttle = lerp(from.throttle, input.throttle, t);
            let speed = lerp(from.speed_mps, input.speed_mps, t);
            let skid = lerp(from.skid, input.skid, t);
            let volume = lerp(from.volume, input.volume, t);
            let tone = input.tone;
            let firing = rpm / 60.0 * input.cylinders.max(1) as f32 / 2.0 * tone.pitch;

            self.phase = (self.phase + (firing * dt) as f64).fract();
            let p = self.phase as f32 * std::f32::consts::TAU;
            let load = throttle.clamp(0.0, 1.0);
            let mut engine = 0.0;
            for (h, (idle, full)) in HARMONICS.iter().enumerate() {
                let rasp = if h == 0 {
                    1.0
                } else {
                    tone.roughness.powf(h as f32 * 0.5)
                };
                engine += lerp(*idle, *full, load) * rasp * (p * (h + 1) as f32).sin();
            }
            // Uneven firing: a rumble at half the note.
            engine += 0.25 * tone.rumble * (p * 0.5).sin();
            // Intake and exhaust hiss, low-passed, louder with throttle and revs.
            let white = self.white();
            let a = (0.05 + rpm / 20000.0).min(0.6);
            self.intake_lp += a * (white - self.intake_lp);
            let rev = (rpm / 6000.0).clamp(0.0, 1.0);
            let intake = self.intake_lp * (0.15 + 0.85 * load) * (0.3 + rev) * tone.intake;
            let engine_level = if rpm > 1.0 { 0.18 + 0.22 * load } else { 0.0 };

            // Road roar: twice low-passed noise rising with speed.
            let white = self.white();
            self.roar_lp += 0.08 * (white - self.roar_lp);
            self.roar_lp2 += 0.08 * (self.roar_lp - self.roar_lp2);
            let roar = self.roar_lp2 * (speed / 30.0).clamp(0.0, 1.0) * 1.6 * tone.road;

            // Squeal: a wavering tone around 900 Hz while sliding.
            let wobble = 1.0 + 0.04 * (self.squeal_phase as f32 * 0.013).sin();
            self.squeal_phase = (self.squeal_phase + (900.0 * wobble * dt) as f64).fract();
            let squeal = tone.skid
                * skid.clamp(0.0, 1.0)
                * ((self.squeal_phase as f32 * std::f32::consts::TAU).sin() * 0.6
                    + self.white() * 0.15);

            let mix = engine * engine_level * 0.45 + intake * 0.6 + roar * 0.5 + squeal * 0.35;
            // Soft clip keeps loud moments from crackling.
            *s = (mix * volume).tanh();
        }
        self.last = *input;
    }
}

/// One sound in the scene: who makes it, what it is, and where it sits
/// left to right for the listener (−1 left … 1 right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    /// Stable id (an entity), so each source keeps its own voice.
    pub key: u64,
    pub input: SoundInput,
    pub pan: f32,
}

/// Mixes many vehicles into stereo. Each source keeps a [`Voice`] for as
/// long as it is heard; a source that stops being listed fades out over
/// one buffer and is dropped.
#[derive(Clone, Debug)]
pub struct Mixer {
    sample_rate: u32,
    voices: Vec<(u64, Voice, f32)>,
    scratch: Vec<f32>,
}

impl Mixer {
    pub fn new(sample_rate: u32) -> Mixer {
        Mixer {
            sample_rate,
            voices: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Render `left.len()` frames of `sources` into `left` and `right`.
    pub fn render(&mut self, sources: &[Source], left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        self.scratch.resize(left.len(), 0.0);
        for src in sources {
            if !self.voices.iter().any(|(k, _, _)| *k == src.key) {
                self.voices
                    .push((src.key, Voice::new(self.sample_rate), src.pan));
            }
        }
        let mut gone = Vec::new();
        for (i, (key, voice, pan)) in self.voices.iter_mut().enumerate() {
            let (input, new_pan) = match sources.iter().find(|s| s.key == *key) {
                Some(s) => (s.input, s.pan),
                None => {
                    gone.push(i);
                    (
                        SoundInput {
                            volume: 0.0,
                            ..voice.last
                        },
                        *pan,
                    )
                }
            };
            voice.render(&input, &mut self.scratch);
            let n = left.len().max(1) as f32;
            for (k, s) in self.scratch.iter().enumerate() {
                let p = *pan + (new_pan - *pan) * (k as f32 + 1.0) / n;
                // Constant-power pan.
                let angle = (p.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
                left[k] += s * angle.cos();
                right[k] += s * angle.sin();
            }
            *pan = new_pan;
        }
        for i in gone.into_iter().rev() {
            self.voices.remove(i);
        }
        for s in left.iter_mut().chain(right.iter_mut()) {
            *s = s.tanh();
        }
    }

    /// Voices currently sounding.
    pub fn voices(&self) -> usize {
        self.voices.len()
    }
}

/// 16-bit mono PCM WAV bytes.
pub fn wav_bytes(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Strength of frequency `hz` in `samples` (Goertzel), for checking sounds.
pub fn tone_power(samples: &[f32], sample_rate: u32, hz: f32) -> f32 {
    let w = std::f32::consts::TAU * hz / sample_rate as f32;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for x in samples {
        let s = x + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2) / samples.len().max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(input: SoundInput, seconds: f32) -> Vec<f32> {
        let mut v = Voice::new(44_100);
        v.render(&input, &mut vec![0.0; 4410]); // settle the glide
        let mut out = vec![0.0; (44_100.0 * seconds) as usize];
        v.render(&input, &mut out);
        out
    }

    #[test]
    fn the_engine_sings_at_its_firing_frequency() {
        let input = SoundInput {
            rpm: 3000.0,
            throttle: 0.5,
            cylinders: 4,
            volume: 1.0,
            ..Default::default()
        };
        assert_eq!(input.firing_hz(), 100.0);
        let s = render(input, 1.0);
        let at = |hz| tone_power(&s, 44_100, hz);
        assert!(
            at(100.0) > 20.0 * at(130.0),
            "{} vs {}",
            at(100.0),
            at(130.0)
        );
        assert!(at(200.0) > 5.0 * at(170.0), "second harmonic");
        // A six-cylinder at the same revs sings half as high again.
        let six = render(
            SoundInput {
                cylinders: 6,
                ..input
            },
            1.0,
        );
        assert!(tone_power(&six, 44_100, 150.0) > 20.0 * tone_power(&six, 44_100, 100.0));
    }

    #[test]
    fn load_speed_and_slides_are_audible() {
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        let base = SoundInput {
            rpm: 2500.0,
            cylinders: 4,
            volume: 1.0,
            ..Default::default()
        };
        let idle = rms(&render(base, 0.5));
        let full = rms(&render(
            SoundInput {
                throttle: 1.0,
                ..base
            },
            0.5,
        ));
        assert!(full > idle * 1.3, "throttle is louder: {full} vs {idle}");
        let fast = render(
            SoundInput {
                speed_mps: 30.0,
                ..base
            },
            0.5,
        );
        assert!(rms(&fast) > idle);
        let skid = render(SoundInput { skid: 1.0, ..base }, 0.5);
        assert!(
            tone_power(&skid, 44_100, 900.0) > 50.0 * tone_power(&render(base, 0.5), 44_100, 900.0)
        );
        // Silent when off.
        assert_eq!(rms(&render(SoundInput::default(), 0.1)), 0.0);
    }

    #[test]
    fn wav_header_is_valid() {
        let w = wav_bytes(&[0.0, 0.5, -0.5], 22_050);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 22_050);
    }

    #[test]
    fn mixer_pans_and_fades_sources() {
        let car = SoundInput {
            rpm: 2000.0,
            throttle: 0.5,
            cylinders: 4,
            volume: 0.8,
            ..Default::default()
        };
        let mut m = Mixer::new(44_100);
        let (mut l, mut r) = (vec![0.0; 4410], vec![0.0; 4410]);
        let left_car = [Source {
            key: 1,
            input: car,
            pan: -1.0,
        }];
        for _ in 0..3 {
            m.render(&left_car, &mut l, &mut r);
        }
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        assert!(
            rms(&l) > 10.0 * rms(&r).max(1e-6),
            "hard left: {} vs {}",
            rms(&l),
            rms(&r)
        );
        assert_eq!(m.voices(), 1);
        // Gone from the list: one buffer to fade, then dropped.
        m.render(&[], &mut l, &mut r);
        assert_eq!(m.voices(), 0);
        m.render(&[], &mut l, &mut r);
        assert_eq!(rms(&l), 0.0);
    }
}
