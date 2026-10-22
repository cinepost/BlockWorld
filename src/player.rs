use crate::{config::*, world::World};
use glam::{IVec3, Vec3};
use std::collections::HashSet;
use winit::keyboard::KeyCode;

pub const HALF_WIDTH: f32 = 0.3;
pub const PLAYER_HEIGHT: f32 = 1.8;
pub struct Player {
    pub auto_jump: bool,
    pub flying: bool,
    pub feet: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    velocity: Vec3,
    grounded: bool,
}
impl Player {
    pub fn new(y: f32) -> Self {
        Self {
            auto_jump: false,
            flying: false,
            feet: Vec3::new(0.5, y, 0.5),
            yaw: 0.5,
            pitch: -0.2,
            velocity: Vec3::ZERO,
            grounded: false,
        }
    }
    pub fn eye(&self) -> Vec3 {
        self.feet + Vec3::Y * 1.62
    }
    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        )
    }
    pub fn right(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin())
    }
    pub fn look(&mut self, dx: f64, dy: f64) {
        self.yaw += dx as f32 * 0.0025;
        self.pitch = (self.pitch - dy as f32 * 0.0025).clamp(-1.54, 1.54);
    }
    pub fn overlaps(&self, p: IVec3) -> bool {
        let min = self.feet - Vec3::new(HALF_WIDTH, 0.0, HALF_WIDTH) + Vec3::splat(0.001);
        let max = self.feet + Vec3::new(HALF_WIDTH, PLAYER_HEIGHT, HALF_WIDTH) - Vec3::splat(0.001);
        min.cmplt(p.as_vec3() + Vec3::ONE).all() && max.cmpgt(p.as_vec3()).all()
    }
    fn collides(&self, w: &World) -> bool {
        let min = (self.feet - Vec3::new(HALF_WIDTH, 0.0, HALF_WIDTH) + Vec3::splat(0.001))
            .floor()
            .as_ivec3();
        let max = (self.feet + Vec3::new(HALF_WIDTH, PLAYER_HEIGHT, HALF_WIDTH)
            - Vec3::splat(0.001))
        .floor()
        .as_ivec3();
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    if w.get(IVec3::new(x, y, z)).solid() {
                        return true;
                    }
                }
            }
        }
        false
    }
    /// Body immersion, rather than eye immersion, keeps swimming active at the surface.
    fn in_water(&self, w: &World) -> bool {
        let min = (self.feet - Vec3::new(HALF_WIDTH, 0.0, HALF_WIDTH) + Vec3::splat(0.001))
            .floor()
            .as_ivec3();
        let max = (self.feet + Vec3::new(HALF_WIDTH, PLAYER_HEIGHT, HALF_WIDTH)
            - Vec3::splat(0.001))
        .floor()
        .as_ivec3();
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    if w.get(IVec3::new(x, y, z)) == crate::world::Block::Water {
                        return true;
                    }
                }
            }
        }
        false
    }
    pub fn toggle_fly(&mut self) {
        self.flying = !self.flying;
        self.velocity = Vec3::ZERO;
        self.grounded = false;
    }
    pub fn update(&mut self, w: &World, keys: &HashSet<KeyCode>, dt: f32) {
        if self.flying {
            let key = |k| if keys.contains(&k) { 1.0 } else { 0.0 };
            let direction = (self.forward() * (key(KeyCode::KeyW) - key(KeyCode::KeyS))
                + self.right() * (key(KeyCode::KeyD) - key(KeyCode::KeyA))
                + Vec3::Y * (key(KeyCode::Space) - key(KeyCode::KeyC)))
            .normalize_or_zero();
            let speed = if keys.contains(&KeyCode::ControlLeft)
                || keys.contains(&KeyCode::ControlRight)
                || keys.contains(&KeyCode::ShiftLeft)
            {
                45.0
            } else {
                12.0
            };
            self.feet += direction * speed * dt;
            return;
        }
        let key = |k| if keys.contains(&k) { 1.0 } else { 0.0 };
        let front = Vec3::new(self.yaw.sin(), 0.0, -self.yaw.cos());
        let move_dir = (front * (key(KeyCode::KeyW) - key(KeyCode::KeyS))
            + self.right() * (key(KeyCode::KeyD) - key(KeyCode::KeyA)))
        .normalize_or_zero();
        let swimming = self.in_water(w);
        let head_underwater = w.get(self.eye().floor().as_ivec3()) == crate::world::Block::Water;
        let speed = if swimming {
            2.8
        } else if keys.contains(&KeyCode::ShiftLeft)
            || keys.contains(&KeyCode::ControlLeft)
            || keys.contains(&KeyCode::ControlRight)
        {
            7.0
        } else {
            4.3
        };
        if !self.grounded && self.velocity.y <= 0.0 {
            self.feet.y -= 0.02;
            self.grounded = self.collides(w);
            self.feet.y += 0.02;
        }
        // Releases do not remove takeoff momentum. A held key can restart a
        // wall-blocked component after clearing the wall (auto-jump / shore exits).
        if self.grounded || swimming {
            self.velocity.x = move_dir.x * speed;
            self.velocity.z = move_dir.z * speed;
        } else {
            let limit = self.velocity.x.hypot(self.velocity.z).max(speed);
            if self.velocity.x.abs() < 0.001 && move_dir.x != 0.0 {
                self.velocity.x = move_dir.x * speed;
            }
            if self.velocity.z.abs() < 0.001 && move_dir.z != 0.0 {
                self.velocity.z = move_dir.z * speed;
            }
            let length = self.velocity.x.hypot(self.velocity.z);
            if length > limit {
                self.velocity.x *= limit / length;
                self.velocity.z *= limit / length;
            }
        }
        if swimming {
            self.velocity.y = if keys.contains(&KeyCode::Space) {
                // A surface jump clears a one-block bank; submerged ascent stays slower.
                if head_underwater {
                    3.5
                } else {
                    8.0
                }
            } else {
                -1.5
            };
        } else {
            let auto_jump = self.auto_jump && self.grounded && move_dir != Vec3::ZERO && {
                let original = self.feet;
                self.feet += move_dir * 0.55;
                let obstacle = self.collides(w);
                self.feet.y += 1.01;
                let destination_clear = !self.collides(w);
                self.feet = original + Vec3::Y * 1.01;
                let overhead_clear = !self.collides(w);
                self.feet = original;
                obstacle && destination_clear && overhead_clear
            };
            if self.grounded && (keys.contains(&KeyCode::Space) || auto_jump) {
                self.velocity.y = 8.0;
            }
            self.velocity.y = (self.velocity.y - 24.0 * dt).max(-35.0);
        }
        self.grounded = false;
        // Substeps prevent tunnelling; resolve each axis separately for wall sliding.
        let steps = (dt / 0.008).ceil().max(1.0) as usize;
        for _ in 0..steps {
            for a in [0, 2, 1] {
                let old = self.feet[a];
                self.feet[a] += self.velocity[a] * dt / steps as f32;
                if self.collides(w) {
                    self.feet[a] = old;
                    if a == 1 && self.velocity.y < 0.0 {
                        self.grounded = true;
                    }
                    self.velocity[a] = 0.0;
                }
            }
        }
        if self.feet.y < MIN_Y as f32 - 10.0 {
            self.feet.y = 80.0;
            self.velocity = Vec3::ZERO;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Block;
    fn pool() -> World {
        let mut w = World::fixture();
        for z in -3..=3 {
            for x in -3..=7 {
                w.set(IVec3::new(x, 99, z), Block::Stone);
                for y in 100..=104 {
                    w.set(
                        IVec3::new(x, y, z),
                        if x >= 3 {
                            Block::Sand
                        } else if y < 104 {
                            Block::Water
                        } else {
                            Block::Air
                        },
                    );
                }
            }
        }
        w
    }
    fn step_world(height: i32, ceiling: bool) -> World {
        let mut w = World::fixture();
        for z in -3..=3 {
            for x in -3..=10 {
                w.set(IVec3::new(x, 99, z), Block::Stone);
                for y in 100..=105 {
                    w.set(
                        IVec3::new(x, y, z),
                        if (x == 2 && y < 100 + height) || (ceiling && y == 102) {
                            Block::Stone
                        } else {
                            Block::Air
                        },
                    );
                }
            }
        }
        w
    }
    #[test]
    fn fly_camera_ignores_collision_and_gravity_and_resets_momentum() {
        let mut w = World::fixture();
        let mut p = Player::new(100.0);
        p.yaw = 0.0;
        p.pitch = 0.0;
        p.velocity = Vec3::splat(5.0);
        p.toggle_fly();
        w.set(IVec3::new(0, 101, -1), Block::Stone);
        p.update(&w, &HashSet::from([KeyCode::KeyW]), 0.5);
        assert!((p.feet.z + 5.5).abs() < 0.001);
        let position = p.feet;
        p.update(&w, &HashSet::new(), 0.5);
        assert_eq!(p.feet, position);
        p.update(
            &w,
            &HashSet::from([KeyCode::Space, KeyCode::ControlLeft]),
            1.0,
        );
        assert_eq!(p.feet.y, 145.0);
        p.toggle_fly();
        assert_eq!(p.velocity, Vec3::ZERO);
    }
    #[test]
    fn airborne_input_cannot_stack_horizontal_speed() {
        let w = step_world(0, false);
        let mut p = Player::new(100.0);
        p.yaw = 0.0;
        let keys = HashSet::from([KeyCode::KeyW, KeyCode::ControlLeft, KeyCode::Space]);
        p.update(&w, &keys, 0.01);
        p.yaw = std::f32::consts::FRAC_PI_2;
        p.update(&w, &keys, 0.01);
        assert!(p.velocity.x.hypot(p.velocity.z) <= 7.001);
    }
    #[test]
    fn releasing_keys_in_air_preserves_takeoff_velocity_until_landing() {
        let w = step_world(0, false);
        let mut p = Player::new(100.0);
        p.yaw = std::f32::consts::FRAC_PI_2;
        p.update(
            &w,
            &HashSet::from([
                KeyCode::KeyW,
                KeyCode::KeyD,
                KeyCode::ControlLeft,
                KeyCode::Space,
            ]),
            0.01,
        );
        let horizontal = Vec3::new(p.velocity.x, 0.0, p.velocity.z);
        assert!(horizontal.length() > 6.9);
        for keys in [HashSet::from([KeyCode::KeyW]), HashSet::new()] {
            p.yaw += 1.0;
            p.update(&w, &keys, 0.05);
            assert!((Vec3::new(p.velocity.x, 0.0, p.velocity.z) - horizontal).length() < 0.001);
        }
        for _ in 0..100 {
            p.update(&w, &HashSet::new(), 0.01);
            if p.grounded {
                break;
            }
        }
        assert!(p.grounded);
        p.update(&w, &HashSet::new(), 0.01);
        assert_eq!(p.velocity.x, 0.0);
        assert_eq!(p.velocity.z, 0.0);
    }
    #[test]
    fn auto_jump_requires_one_block_and_clearance() {
        for (enabled, height, ceiling, passes) in [
            (true, 1, false, true),
            (false, 1, false, false),
            (true, 2, false, false),
            (true, 1, true, false),
        ] {
            let w = step_world(height, ceiling);
            let mut p = Player::new(100.0);
            p.yaw = std::f32::consts::FRAC_PI_2;
            p.auto_jump = enabled;
            for _ in 0..200 {
                p.update(&w, &HashSet::from([KeyCode::KeyW]), 0.01);
                assert!(!p.collides(&w));
            }
            assert_eq!(
                p.feet.x > 3.3,
                passes,
                "enabled {enabled}, height {height}, ceiling {ceiling}, {:?}",
                p.feet
            );
        }
    }
    #[test]
    fn either_control_key_runs() {
        let w = step_world(0, false);
        for key in [KeyCode::ControlLeft, KeyCode::ControlRight] {
            let mut walk = Player::new(100.0);
            walk.yaw = std::f32::consts::FRAC_PI_2;
            let mut run = Player::new(100.0);
            run.yaw = walk.yaw;
            for _ in 0..50 {
                walk.update(&w, &HashSet::from([KeyCode::KeyW]), 0.01);
                run.update(&w, &HashSet::from([KeyCode::KeyW, key]), 0.01);
            }
            assert!((walk.feet.x - 0.5 - 2.15).abs() < 0.01);
            assert!((run.feet.x - 0.5 - 3.5).abs() < 0.01);
        }
    }
    #[test]
    fn swim_with_head_above_surface() {
        let w = pool();
        let mut p = Player::new(102.5);
        assert_ne!(w.get(p.eye().floor().as_ivec3()), Block::Water);
        let keys = HashSet::from([KeyCode::Space]);
        for _ in 0..40 {
            p.update(&w, &keys, 0.01);
        }
        assert!(
            p.feet.y > 104.0,
            "Feet must clear the water while holding Space"
        );
    }
    #[test]
    fn swim_out_onto_one_block_bank() {
        let w = pool();
        let mut p = Player::new(100.1);
        p.yaw = std::f32::consts::FRAC_PI_2;
        let keys = HashSet::from([KeyCode::KeyW, KeyCode::Space]);
        let mut landed = false;
        for _ in 0..250 {
            p.update(&w, &keys, 0.01);
            assert!(!p.collides(&w));
            if p.feet.x > 3.3 && p.grounded && p.feet.y > 104.99 {
                landed = true;
                break;
            }
        }
        assert!(
            landed,
            "Player must swim/jump out onto a bank above the surface; feet {:?}",
            p.feet
        );
    }
    #[test]
    fn swim_cannot_pass_through_ceiling() {
        let mut w = pool();
        for z in -2..=2 {
            for x in -2..=2 {
                w.set(IVec3::new(x, 105, z), Block::Stone);
            }
        }
        let mut p = Player::new(101.0);
        let keys = HashSet::from([KeyCode::Space]);
        for _ in 0..200 {
            p.update(&w, &keys, 0.01);
            assert!(!p.collides(&w));
            assert!(p.feet.y + PLAYER_HEIGHT <= 105.001);
        }
    }
    #[test]
    fn landing_and_placement_overlap() {
        let mut w = World::fixture();
        for z in -2..=2 {
            for x in -2..=2 {
                w.set(IVec3::new(x, 99, z), Block::Stone);
            }
        }
        let mut p = Player::new(102.0);
        for _ in 0..200 {
            p.update(&w, &HashSet::new(), 0.01);
        }
        assert!((p.feet.y - 100.0).abs() < 0.01);
        assert!(p.grounded);
        assert!(p.overlaps(IVec3::new(0, 100, 0)));
        assert!(!p.overlaps(IVec3::new(0, 99, 0)));
    }
}
