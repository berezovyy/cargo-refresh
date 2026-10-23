//! Movement, lifecycle, interactions and score accounting.
use bevy::prelude::*;
use sim_core::*;

pub fn probe_label() -> &'static str {
    "S1:v1"
}

pub fn score_reward(base: u64) -> u64 {
    base + 1 // S2: one bonus point for every pickup
}

fn spawn_drone(commands: &mut Commands, arena: &mut Arena, ticks: u64) {
    let serial = arena.next_serial;
    arena.next_serial += 1;
    commands.spawn((
        Drone {
            serial,
            role: (serial % 6) as u8,
        },
        Position(spawn_point(serial)),
        Velocity::default(),
        Body {
            radius: 0.5,
            mass: 1.0 + (serial % 3) as f32,
        },
        Navigation::default(),
        Travel::default(),
        Lifetime {
            born: ticks,
            ttl: 2500 + serial % 500,
        },
    ));
}

pub fn setup(mut commands: Commands, mut arena: ResMut<Arena>) {
    commands.spawn((
        Captain,
        Position::default(),
        Velocity(Vec2::new(1.0, 0.5)),
        Body {
            radius: 1.5,
            mass: 4.0,
        },
        Travel::default(),
        PilotStatus {
            energy: 100,
            ..Default::default()
        },
    ));
    for _ in 0..48 {
        spawn_drone(&mut commands, &mut arena, 0);
    }
    // A central pickup guarantees observable score even before the first wave.
    for index in 0..24 {
        let angle = index as f32 * std::f32::consts::TAU / 24.0;
        commands.spawn((
            Collectible { points: 10 },
            Position(Vec2::new(angle.cos(), angle.sin()) * 3.0),
            Lifetime { born: 0, ttl: 1500 },
        ));
    }
}

pub fn advance_clock(mut clock: ResMut<Clock>, mut arena: ResMut<Arena>) {
    clock.ticks += 1;
    let phase = clock.ticks as f32 * STEP;
    arena.beacon = Vec2::new((phase * 0.3).cos(), (phase * 0.2).sin()) * 12.0;
    arena.threat = (arena.wave as f32 * 0.03).min(2.0);
}

pub fn apply_controls(
    queue: Res<ControlQueue>,
    clock: Res<Clock>,
    mut arena: ResMut<Arena>,
    mut score: ResMut<Score>,
    mut commands: Commands,
    mut captain: Query<(&mut Velocity, &mut PilotStatus), With<Captain>>,
) {
    let controls: Vec<_> = queue.0.lock().unwrap().drain(..).collect();
    for control in controls {
        score.commands += 1;
        match control {
            Control::Boost => {
                if let Ok((mut velocity, mut pilot)) = captain.single_mut() {
                    velocity.0 += Vec2::new(2.0, 1.0);
                    pilot.energy = pilot.energy.saturating_sub(5);
                }
            }
            Control::Spawn => spawn_drone(&mut commands, &mut arena, clock.ticks),
            Control::Reward => score.total += score_reward(100),
        }
    }
}

pub fn spawn_wave(
    mut commands: Commands,
    clock: Res<Clock>,
    mut arena: ResMut<Arena>,
    mut waves: MessageWriter<WaveStarted>,
) {
    if !clock.ticks.is_multiple_of(50) {
        return;
    }
    arena.wave += 1;
    for _ in 0..3 {
        spawn_drone(&mut commands, &mut arena, clock.ticks);
    }
    let point = spawn_point(arena.next_serial + clock.ticks) * 0.3;
    commands.spawn((
        Collectible {
            points: 10 + arena.wave,
        },
        Position(point),
        Lifetime {
            born: clock.ticks,
            ttl: 1500,
        },
    ));
    waves.write(WaveStarted { wave: arena.wave });
}

pub fn retarget(
    clock: Res<Clock>,
    arena: Res<Arena>,
    mut drones: Query<(&Drone, &Position, &mut Navigation)>,
) {
    for (drone, position, mut navigation) in &mut drones {
        let phase = (clock.ticks + drone.serial * 11) as f32 * 0.015;
        navigation.target = match drone.role {
            0 => arena.beacon,
            1 => -arena.beacon,
            2 => Vec2::new(phase.cos(), phase.sin()) * 20.0,
            3 => Vec2::new(-position.0.y, position.0.x) * 0.8,
            4 => spawn_point(drone.serial + clock.ticks / 100),
            _ => arena.beacon * 0.5 + Vec2::new(phase.sin(), phase.cos()) * 8.0,
        };
        navigation.acceleration = Vec2::ZERO;
    }
}

pub fn steering(
    arena: Res<Arena>,
    mut drones: Query<(&Position, &mut Velocity, &Navigation, &Body), With<Drone>>,
) {
    for (position, mut velocity, navigation, body) in &mut drones {
        let acceleration = steer(position.0, velocity.0, navigation.target, 0.8)
            + navigation.acceleration
            + arena.wind;
        velocity.0 = limited(velocity.0 + acceleration * STEP / body.mass, speed_limit());
    }
}

pub fn movement(
    mut moving: Query<(&mut Position, &Velocity, &mut Travel)>,
    mut score: ResMut<Score>,
) {
    for (mut position, velocity, mut travel) in &mut moving {
        let delta = velocity.0 * STEP;
        position.0 += delta;
        let distance = f64::from(delta.length());
        travel.distance += distance;
        score.distance += distance;
    }
}

pub fn collisions(
    mut bodies: Query<(Entity, &mut Position, &mut Velocity, &Body)>,
    mut events: MessageWriter<Collision>,
) {
    for (entity, mut position, mut velocity, body) in &mut bodies {
        let edge = ARENA_HALF_SIZE - body.radius;
        for axis in 0..2 {
            if position.0[axis].abs() > edge {
                let impulse = velocity.0[axis].abs() * body.mass;
                position.0[axis] = position.0[axis].clamp(-edge, edge);
                velocity.0[axis] *= -0.85;
                events.write(Collision { entity, impulse });
            }
        }
    }
}

pub fn collection(
    pickups: Query<(Entity, &Position, &Collectible)>,
    collectors: Query<(&Position, &Body), Without<Collectible>>,
    mut events: MessageWriter<Collected>,
) {
    for (entity, point, pickup) in &pickups {
        if collectors.iter().any(|(position, body)| {
            position.0.distance_squared(point.0) < (body.radius + 0.8).powi(2)
        }) {
            events.write(Collected {
                entity,
                points: pickup.points,
            });
        }
    }
}

pub fn scoring(
    mut commands: Commands,
    mut score: ResMut<Score>,
    mut collected: MessageReader<Collected>,
    mut collisions: MessageReader<Collision>,
    mut waves: MessageReader<WaveStarted>,
) {
    for event in collected.read() {
        score.total += score_reward(event.points);
        score.collected += 1;
        commands.entity(event.entity).despawn();
    }
    for event in collisions.read() {
        score.collisions += 1;
        // Read the event payload: stronger wall hits grant a tiny consolation bonus.
        score.total += (event.impulse / 20.0) as u64;
    }
    for event in waves.read() {
        score.total += event.wave;
    }
}

pub fn cleanup(
    mut commands: Commands,
    clock: Res<Clock>,
    aging: Query<(Entity, &Lifetime)>,
    mut pilots: Query<&mut PilotStatus>,
) {
    for (entity, lifetime) in &aging {
        if clock.ticks.saturating_sub(lifetime.born) >= lifetime.ttl {
            commands.entity(entity).despawn();
        }
    }
    if clock.ticks.is_multiple_of(100) {
        for mut pilot in &mut pilots {
            pilot.energy = (pilot.energy + 1).min(100);
        }
    }
}
