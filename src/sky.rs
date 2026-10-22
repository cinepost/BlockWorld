//! Double-buffered GPU sky scheduling. Importance entries include solid-angle PDFs.
use glam::Vec3;
pub const SKY_W: usize = 1024;
pub const SKY_H: usize = 512;
pub const IMPORTANCE_W: usize = 256;
pub const IMPORTANCE_H: usize = 128;
pub const BANK_ENTRIES: usize = SKY_W * SKY_H + IMPORTANCE_W * IMPORTANCE_H + IMPORTANCE_H + 129 * 10 + 130;
pub const CACHE_BYTES: usize = BANK_ENTRIES * 2 * 16;
pub struct SkyCache {
    pub active: usize,
    pub write_bank: usize,
    pub ready: bool,
    pub build_sun: Vec3,
    pub build_height: f32,
    pub rebuilds: u64,
    pub interval: f32,
    row: u32,
    published_sun: Vec3,
    published_height: f32,
    last_time: f32,
    building: bool,
}
impl Default for SkyCache {
    fn default() -> Self {
        Self {
            active: 0,
            write_bank: 0,
            ready: false,
            build_sun: Vec3::Y,
            build_height: 1.0,
            rebuilds: 0,
            interval: 0.0,
            row: 0,
            published_sun: Vec3::Y,
            published_height: 0.0,
            last_time: f32::NEG_INFINITY,
            building: false,
        }
    }
}
impl SkyCache {
    pub fn progress(&self) -> u32 {
        if self.building {
            self.row
        } else {
            SKY_H as u32
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn advance(
        &mut self,
        sun: Vec3,
        height: f32,
        time: f32,
        enabled: bool,
        cycle: bool,
        day: f32,
        headless: bool,
    ) -> Option<(u32, u32, bool)> {
        if !enabled {
            return None;
        }
        let horizon = sun.y.abs() < 0.15;
        let angular_budget = if horizon { 0.04f32 } else { 0.18f32 }.to_radians();
        self.interval = if cycle {
            (angular_budget / (std::f32::consts::TAU / day.max(30.0))).clamp(0.05, 30.0)
        } else {
            30.0
        };
        let altitude = (1000.0 + height).clamp(50.0, 10000.0);
        let altitude_changed = (altitude - self.published_height).abs() > 50.0;
        let changed = self.published_sun.dot(sun).clamp(-1.0, 1.0).acos() > angular_budget
            || altitude_changed;
        if !self.building
            && (!self.ready
                || (changed
                    && (!cycle
                        || altitude_changed
                        || (time - self.last_time).abs() >= self.interval)))
        {
            self.building = true;
            self.last_time = time;
            self.row = 0;
            self.write_bank = if self.ready { 1 - self.active } else { 0 };
            self.build_sun = sun;
            self.build_height = altitude;
        }
        if !self.building {
            return None;
        }
        let first = self.row;
        let count = if !self.ready || headless {
            SKY_H as u32
        } else {
            64
        }
        .min(SKY_H as u32 - first);
        self.row += count;
        let finish = self.row == SKY_H as u32;
        if finish {
            self.active = self.write_bank;
            self.ready = true;
            self.building = false;
            self.rebuilds += 1;
            self.published_sun = self.build_sun;
            self.published_height = self.build_height;
        }
        Some((first, count, finish))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_publishes_complete_banks_and_adapts_at_horizon() {
        let mut c = SkyCache::default();
        let a = Vec3::new(0.0, 0.5, 0.8660254).normalize();
        assert_eq!(
            c.advance(a, 10.0, 0.0, true, true, 600.0, false),
            Some((0, 512, true))
        );
        assert!(c.ready);
        let old = c.active;
        let interval = c.interval;
        assert!(c.advance(a, 10.0, 1.0, true, true, 600.0, false).is_none());
        let b = Vec3::new(0.0, 0.05, 1.0).normalize();
        assert_eq!(
            c.advance(b, 10.0, 2.0, true, true, 600.0, false),
            Some((0, 64, false))
        );
        assert_eq!(c.active, old);
        assert!(c.interval < interval);
        for _ in 0..7 {
            c.advance(b, 10.0, 2.0, true, true, 600.0, false);
        }
        assert_ne!(c.active, old);
        assert_eq!(c.rebuilds, 2);
    }
    #[test]
    fn manual_light_changes_do_not_wait_for_cycle_interval() {
        let mut c = SkyCache::default();
        c.advance(Vec3::Y, 0.0, 0.0, true, false, 600.0, false);
        assert_eq!(
            c.advance(Vec3::X, 0.0, 0.001, true, false, 600.0, false),
            Some((0, 64, false))
        );
        assert_eq!(c.progress(), 64);
    }
}
