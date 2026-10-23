//! Encounter steering rules. S7 inserts a comment before this module doc.
//! Each active rule contributes a bounded acceleration in a spatial zone.
use bevy::prelude::*;
use sim_core::*;

pub fn apply_encounters(
    clock: Res<Clock>,
    mut arena: ResMut<Arena>,
    mut drones: Query<(&Drone, &Position, &Velocity, &mut Navigation)>,
) {
    arena.wind = Vec2::new((clock.ticks as f32 * 0.003).sin() * 0.2, 0.05);
    for (drone, position, velocity, mut navigation) in &mut drones {
        let mut total = Vec2::ZERO;
        total += harbor(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += crosswind(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += gravity_well(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += slalom(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += ring(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += convoy(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += ambush(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += escape(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += pursuit(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += spiral(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += minefield(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += checkpoint(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += refinery(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += storm(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += relay(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += bottleneck(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += sentinel(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += drift(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += magnet(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += oscillator(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += funnel(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += sweep(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += orbit(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += pulse(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += gauntlet(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += rescue(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += intercept(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += patrol(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += vortex(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += shelter(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += outpost(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += homecoming(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += surge(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += lighthouse(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += canyon(drone, position.0, velocity.0, clock.ticks, arena.threat);
        total += switchback(drone, position.0, velocity.0, clock.ticks, arena.threat);
        navigation.acceleration = limited(total, 4.0);
        if total.length_squared() > 0.0001 {
            arena.rule_activations += 1;
        }
    }
}

/// Harbor encounter: spatial steering with a timed recovery phase.
fn harbor(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 0;
    let cycle = (ticks + drone.serial * 3) % 180;
    let active = cycle < 80;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-30.0, -30.0);
    let offset = position - center;
    let radius = 14.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.2;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 180.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Crosswind encounter: spatial steering with a timed recovery phase.
fn crosswind(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 1;
    let cycle = (ticks + drone.serial * 4) % 191;
    let active = cycle < 83;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-18.0, -30.0);
    let offset = position - center;
    let radius = 15.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.35;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 191.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Gravity well encounter: spatial steering with a timed recovery phase.
fn gravity_well(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 2;
    let cycle = (ticks + drone.serial * 5) % 202;
    let active = cycle < 86;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-6.0, -30.0);
    let offset = position - center;
    let radius = 16.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.5;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 202.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Slalom encounter: spatial steering with a timed recovery phase.
fn slalom(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 3;
    let cycle = (ticks + drone.serial * 6) % 213;
    let active = cycle < 89;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(6.0, -30.0);
    let offset = position - center;
    let radius = 17.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.65;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 213.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Ring encounter: spatial steering with a timed recovery phase.
fn ring(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 4;
    let cycle = (ticks + drone.serial * 7) % 224;
    let active = cycle < 92;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(18.0, -30.0);
    let offset = position - center;
    let radius = 18.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.8;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 224.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Convoy encounter: spatial steering with a timed recovery phase.
fn convoy(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 5;
    let cycle = (ticks + drone.serial * 8) % 235;
    let active = cycle < 95;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(30.0, -30.0);
    let offset = position - center;
    let radius = 19.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.2;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 235.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Ambush encounter: spatial steering with a timed recovery phase.
fn ambush(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 0;
    let cycle = (ticks + drone.serial * 9) % 246;
    let active = cycle < 98;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-30.0, -18.0);
    let offset = position - center;
    let radius = 20.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.35;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 246.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Escape encounter: spatial steering with a timed recovery phase.
fn escape(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 1;
    let cycle = (ticks + drone.serial * 10) % 257;
    let active = cycle < 101;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-18.0, -18.0);
    let offset = position - center;
    let radius = 21.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.5;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 257.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Pursuit encounter: spatial steering with a timed recovery phase.
fn pursuit(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 2;
    let cycle = (ticks + drone.serial * 11) % 268;
    let active = cycle < 104;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-6.0, -18.0);
    let offset = position - center;
    let radius = 22.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.65;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 268.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Spiral encounter: spatial steering with a timed recovery phase.
fn spiral(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 3;
    let cycle = (ticks + drone.serial * 12) % 279;
    let active = cycle < 107;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(6.0, -18.0);
    let offset = position - center;
    let radius = 14.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.8;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 279.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Minefield encounter: spatial steering with a timed recovery phase.
fn minefield(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 4;
    let cycle = (ticks + drone.serial * 13) % 290;
    let active = cycle < 110;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(18.0, -18.0);
    let offset = position - center;
    let radius = 15.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.2;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 290.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Checkpoint encounter: spatial steering with a timed recovery phase.
fn checkpoint(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 5;
    let cycle = (ticks + drone.serial * 14) % 301;
    let active = cycle < 113;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(30.0, -18.0);
    let offset = position - center;
    let radius = 16.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.35;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 301.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Refinery encounter: spatial steering with a timed recovery phase.
fn refinery(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 0;
    let cycle = (ticks + drone.serial * 15) % 312;
    let active = cycle < 116;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-30.0, -6.0);
    let offset = position - center;
    let radius = 17.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.5;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 312.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Storm encounter: spatial steering with a timed recovery phase.
fn storm(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 1;
    let cycle = (ticks + drone.serial * 16) % 323;
    let active = cycle < 119;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-18.0, -6.0);
    let offset = position - center;
    let radius = 18.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.65;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 323.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Relay encounter: spatial steering with a timed recovery phase.
fn relay(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 2;
    let cycle = (ticks + drone.serial * 17) % 334;
    let active = cycle < 122;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-6.0, -6.0);
    let offset = position - center;
    let radius = 19.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.8;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 334.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Bottleneck encounter: spatial steering with a timed recovery phase.
fn bottleneck(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 3;
    let cycle = (ticks + drone.serial * 18) % 345;
    let active = cycle < 125;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(6.0, -6.0);
    let offset = position - center;
    let radius = 20.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.2;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 345.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Sentinel encounter: spatial steering with a timed recovery phase.
fn sentinel(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 4;
    let cycle = (ticks + drone.serial * 19) % 356;
    let active = cycle < 128;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(18.0, -6.0);
    let offset = position - center;
    let radius = 21.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.35;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 356.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Drift encounter: spatial steering with a timed recovery phase.
fn drift(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 5;
    let cycle = (ticks + drone.serial * 20) % 367;
    let active = cycle < 131;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(30.0, -6.0);
    let offset = position - center;
    let radius = 22.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.5;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 367.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Magnet encounter: spatial steering with a timed recovery phase.
fn magnet(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 0;
    let cycle = (ticks + drone.serial * 21) % 378;
    let active = cycle < 134;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-30.0, 6.0);
    let offset = position - center;
    let radius = 14.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.65;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 378.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Oscillator encounter: spatial steering with a timed recovery phase.
fn oscillator(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 1;
    let cycle = (ticks + drone.serial * 22) % 389;
    let active = cycle < 137;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-18.0, 6.0);
    let offset = position - center;
    let radius = 15.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.8;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 389.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Funnel encounter: spatial steering with a timed recovery phase.
fn funnel(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 2;
    let cycle = (ticks + drone.serial * 23) % 400;
    let active = cycle < 140;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-6.0, 6.0);
    let offset = position - center;
    let radius = 16.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.2;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 400.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Sweep encounter: spatial steering with a timed recovery phase.
fn sweep(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 3;
    let cycle = (ticks + drone.serial * 24) % 411;
    let active = cycle < 143;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(6.0, 6.0);
    let offset = position - center;
    let radius = 17.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.35;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 411.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Orbit encounter: spatial steering with a timed recovery phase.
fn orbit(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 4;
    let cycle = (ticks + drone.serial * 25) % 422;
    let active = cycle < 146;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(18.0, 6.0);
    let offset = position - center;
    let radius = 18.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.5;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 422.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Pulse encounter: spatial steering with a timed recovery phase.
fn pulse(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 5;
    let cycle = (ticks + drone.serial * 26) % 433;
    let active = cycle < 149;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(30.0, 6.0);
    let offset = position - center;
    let radius = 19.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.65;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 433.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Gauntlet encounter: spatial steering with a timed recovery phase.
fn gauntlet(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 0;
    let cycle = (ticks + drone.serial * 27) % 444;
    let active = cycle < 152;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-30.0, 18.0);
    let offset = position - center;
    let radius = 20.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.8;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 444.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Rescue encounter: spatial steering with a timed recovery phase.
fn rescue(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 1;
    let cycle = (ticks + drone.serial * 28) % 455;
    let active = cycle < 155;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-18.0, 18.0);
    let offset = position - center;
    let radius = 21.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.2;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 455.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Intercept encounter: spatial steering with a timed recovery phase.
fn intercept(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 2;
    let cycle = (ticks + drone.serial * 29) % 466;
    let active = cycle < 158;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-6.0, 18.0);
    let offset = position - center;
    let radius = 22.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.35;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 466.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Patrol encounter: spatial steering with a timed recovery phase.
fn patrol(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 3;
    let cycle = (ticks + drone.serial * 30) % 477;
    let active = cycle < 161;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(6.0, 18.0);
    let offset = position - center;
    let radius = 14.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.5;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 477.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Vortex encounter: spatial steering with a timed recovery phase.
fn vortex(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 4;
    let cycle = (ticks + drone.serial * 31) % 488;
    let active = cycle < 164;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(18.0, 18.0);
    let offset = position - center;
    let radius = 15.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.65;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 488.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Shelter encounter: spatial steering with a timed recovery phase.
fn shelter(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 5;
    let cycle = (ticks + drone.serial * 32) % 499;
    let active = cycle < 167;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(30.0, 18.0);
    let offset = position - center;
    let radius = 16.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.8;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 499.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Outpost encounter: spatial steering with a timed recovery phase.
fn outpost(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 0;
    let cycle = (ticks + drone.serial * 33) % 510;
    let active = cycle < 170;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-30.0, 30.0);
    let offset = position - center;
    let radius = 17.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.2;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 510.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Homecoming encounter: spatial steering with a timed recovery phase.
fn homecoming(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 1;
    let cycle = (ticks + drone.serial * 34) % 521;
    let active = cycle < 173;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-18.0, 30.0);
    let offset = position - center;
    let radius = 18.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.35;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 521.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Surge encounter: spatial steering with a timed recovery phase.
fn surge(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 2;
    let cycle = (ticks + drone.serial * 35) % 532;
    let active = cycle < 176;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(-6.0, 30.0);
    let offset = position - center;
    let radius = 19.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.5;
    let circulation = tangent * 0.3;
    let avoidance = if distance < 2.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 532.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Lighthouse encounter: spatial steering with a timed recovery phase.
fn lighthouse(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 3;
    let cycle = (ticks + drone.serial * 36) % 543;
    let active = cycle < 179;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(6.0, 30.0);
    let offset = position - center;
    let radius = 20.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.65;
    let circulation = tangent * -0.4;
    let avoidance = if distance < 3.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.01;
    let phase = cycle as f32 / 543.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Canyon encounter: spatial steering with a timed recovery phase.
fn canyon(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 4;
    let cycle = (ticks + drone.serial * 37) % 554;
    let active = cycle < 182;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(18.0, 30.0);
    let offset = position - center;
    let radius = 21.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.8;
    let circulation = tangent * 0.5;
    let avoidance = if distance < 4.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.02;
    let phase = cycle as f32 / 554.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}

/// Switchback encounter: spatial steering with a timed recovery phase.
fn switchback(drone: &Drone, position: Vec2, velocity: Vec2, ticks: u64, threat: f32) -> Vec2 {
    // Role and time gates avoid every encounter affecting every ship at once.
    let role_matches = drone.role == 5;
    let cycle = (ticks + drone.serial * 38) % 565;
    let active = cycle < 185;
    if !role_matches || !active {
        return Vec2::ZERO;
    }

    let center = Vec2::new(30.0, 30.0);
    let offset = position - center;
    let radius = 22.0;
    let distance = offset.length();
    if distance > radius {
        return Vec2::ZERO;
    }

    // Blend a tangent with attraction; the inner zone repels crowded actors.
    let normal = offset.normalize_or_zero();
    let tangent = Vec2::new(-normal.y, normal.x);
    let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
    let attraction = -normal * 0.2;
    let circulation = tangent * -0.6;
    let avoidance = if distance < 5.0 {
        normal * (1.0 + threat * 0.2)
    } else {
        Vec2::ZERO
    };

    // Damping is deliberately velocity-sensitive, unlike the target planner.
    let damping = velocity * 0.03;
    let phase = cycle as f32 / 565.0;
    let envelope = (phase * std::f32::consts::PI).sin().abs();
    let desired = attraction + circulation + avoidance - damping;
    let strength = falloff * envelope * (1.0 + threat * 0.1);
    let acceleration = desired * strength;

    // Emergency braking bounds kinetic energy near each encounter's center.
    if distance < 1.0 && velocity.length_squared() > 25.0 {
        return limited(acceleration - velocity * 0.15, 1.5);
    }
    limited(acceleration, 1.5)
}
