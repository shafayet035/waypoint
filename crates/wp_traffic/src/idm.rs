//! The Intelligent Driver Model (Treiber, Hennecke & Helbing, 2000): the
//! acceleration a driver chooses from their speed, the speed they want, and
//! the gap and closing speed to the vehicle ahead.

/// A driver's IDM parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Idm {
    /// Comfortable acceleration, m/s².
    pub accel: f32,
    /// Comfortable deceleration, m/s².
    pub decel: f32,
    /// Time gap kept to the vehicle ahead, s.
    pub headway_s: f32,
    /// Gap kept when stopped, m.
    pub min_gap_m: f32,
}

impl Default for Idm {
    /// Typical urban car driver.
    fn default() -> Self {
        Self {
            accel: 1.5,
            decel: 2.0,
            headway_s: 1.4,
            min_gap_m: 2.5,
        }
    }
}

impl Idm {
    /// Acceleration (m/s²) at speed `v`, wanting `v0`, with `gap` metres to
    /// the vehicle ahead closing at `dv` m/s (`f32::INFINITY` gap: free road).
    pub fn accel(&self, v: f32, v0: f32, gap: f32, dv: f32) -> f32 {
        let v = v.max(0.0);
        let free = if v0 > 0.1 {
            1.0 - (v / v0).powi(4)
        } else {
            -1.0
        };
        if !gap.is_finite() {
            return self.accel * free;
        }
        let wanted = self.min_gap_m
            + (v * self.headway_s + v * dv / (2.0 * (self.accel * self.decel).sqrt())).max(0.0);
        self.accel * (free - (wanted / gap.max(0.1)).powi(2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_road_accelerates_to_the_wanted_speed() {
        let idm = Idm::default();
        assert!((idm.accel(0.0, 14.0, f32::INFINITY, 0.0) - 1.5).abs() < 1e-6);
        assert!(idm.accel(14.0, 14.0, f32::INFINITY, 0.0).abs() < 1e-6);
    }

    #[test]
    fn closing_on_a_stopped_car_brakes_harder_the_closer_it_is() {
        let idm = Idm::default();
        let far = idm.accel(14.0, 14.0, 80.0, 14.0);
        let near = idm.accel(14.0, 14.0, 30.0, 14.0);
        assert!(far < 0.0 && near < far, "{far} {near}");
        // Stopped behind a stopped car at the minimum gap: hold still.
        assert!(idm.accel(0.0, 14.0, 2.5, 0.0) <= 0.0);
    }

    #[test]
    fn following_settles_near_the_time_gap() {
        // Simulate a follower behind a leader at a steady 10 m/s.
        let idm = Idm::default();
        let (mut x, mut v, mut lead) = (0.0f32, 10.0f32, 50.0f32);
        for _ in 0..6000 {
            let a = idm.accel(v, 14.0, lead - x - 4.5, v - 10.0);
            v = (v + a * 0.01).max(0.0);
            x += v * 0.01;
            lead += 10.0 * 0.01;
        }
        let gap = lead - x - 4.5;
        assert!((v - 10.0).abs() < 0.05, "matches speed: {v}");
        assert!(gap > 2.5 + 14.0 && gap < 30.0, "gap {gap} m");
    }
}
