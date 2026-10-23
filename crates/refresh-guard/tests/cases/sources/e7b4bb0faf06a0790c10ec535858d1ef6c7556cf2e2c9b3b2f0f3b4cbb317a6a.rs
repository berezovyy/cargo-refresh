//! Shared ECS types and deterministic math, independent of gameplay systems.
use bevy::prelude::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, RwLock};

pub const STEP: f32 = 0.02;
pub const ARENA_HALF_SIZE: f32 = 48.0;

#[derive(Component, Reflect, Clone, Copy, Default)]
#[reflect(Component)]
pub struct Position(pub Vec2);

#[derive(Component, Reflect, Clone, Copy, Default)]
#[reflect(Component)]
pub struct Velocity(pub Vec2);

#[derive(Component, Reflect, Clone, Copy)]
#[reflect(Component)]
pub struct Body {
    pub radius: f32,
    pub mass: f32,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct PilotStatus {
    pub energy: u32,
    pub shield: u32,
}

#[derive(Component)]
pub struct Captain;

#[derive(Component)]
pub struct Drone {
    pub serial: u64,
    pub role: u8,
}

#[derive(Component, Default)]
pub struct Navigation {
    pub target: Vec2,
    pub acceleration: Vec2,
}

#[derive(Component)]
pub struct Lifetime {
    pub born: u64,
    pub ttl: u64,
}

#[derive(Component)]
pub struct Collectible {
    pub points: u64,
}

#[derive(Component, Default)]
pub struct Travel {
    pub distance: f64,
}

#[derive(Resource, Default)]
pub struct Clock {
    pub ticks: u64,
}

#[derive(Resource, Default)]
pub struct Score {
    pub total: u64,
    pub collected: u64,
    pub collisions: u64,
    pub commands: u64,
    pub distance: f64,
}

#[derive(Resource)]
pub struct Arena {
    pub seed: u64,
    pub next_serial: u64,
    pub wave: u64,
    pub threat: f32,
    pub wind: Vec2,
    pub beacon: Vec2,
    pub rule_activations: u64,
}

impl Default for Arena {
    fn default() -> Self {
        Self {
            seed: 0x5eed,
            next_serial: 0,
            wave: 1,
            threat: 0.0,
            wind: Vec2::ZERO,
            beacon: Vec2::new(4.0, 0.0),
            rule_activations: 0,
        }
    }
}

#[derive(Message)]
pub struct Collision {
    pub entity: Entity,
    pub impulse: f32,
}

#[derive(Message)]
pub struct Collected {
    pub entity: Entity,
    pub points: u64,
}

#[derive(Message)]
pub struct WaveStarted {
    pub wave: u64,
}

#[derive(Clone, Copy)]
pub enum Control {
    Boost,
    Spawn,
    Reward,
}

#[derive(Resource, Clone, Default)]
pub struct ControlQueue(pub Arc<Mutex<VecDeque<Control>>>);

#[derive(Resource, Clone, Default)]
pub struct SharedSnapshot(pub Arc<RwLock<String>>);

pub fn speed_limit() -> f32 {
    8.0 // S3: maximum steering speed, also sampled by the probe
}

pub fn limited(vector: Vec2, maximum: f32) -> Vec2 {
    vector.clamp_length_max(maximum)
}

pub fn steer(position: Vec2, velocity: Vec2, target: Vec2, gain: f32) -> Vec2 {
    let desired = (target - position).normalize_or_zero() * speed_limit();
    limited((desired - velocity) * gain, 12.0)
}

pub fn seeded_unit(seed: u64) -> f32 {
    let mut value = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    value ^= value >> 33;
    ((value & 0xffff) as f32) / 65535.0
}

pub fn spawn_point(serial: u64) -> Vec2 {
    let angle = seeded_unit(serial + 17) * std::f32::consts::TAU;
    Vec2::new(angle.cos(), angle.sin()) * (12.0 + seeded_unit(serial) * 25.0)
}

pub fn reflected_fields(value: &dyn Reflect) -> String {
    match value.reflect_ref() {
        bevy::reflect::ReflectRef::Struct(value) => (0..value.field_len())
            .filter_map(|index| value.name_at(index))
            .collect::<Vec<_>>()
            .join(","),
        _ => "opaque".into(),
    }
}

/// Read live component storage through reflection, including fields added by S4.
pub fn reflected_u32_values(value: &dyn Reflect) -> String {
    match value.reflect_ref() {
        bevy::reflect::ReflectRef::Struct(value) => (0..value.field_len())
            .filter_map(|index| {
                let name = value.name_at(index)?;
                let field = value.field_at(index)?.try_downcast_ref::<u32>()?;
                Some(format!("{name}:{field}"))
            })
            .collect::<Vec<_>>()
            .join(","),
        _ => "opaque".into(),
    }
}
