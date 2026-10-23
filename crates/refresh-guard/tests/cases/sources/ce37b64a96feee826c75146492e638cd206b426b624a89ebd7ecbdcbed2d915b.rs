//! Serde work stays in the gameplay crate to exercise derive invalidation.
use bevy::prelude::*;
use serde::Serialize;
use sim_core::*;

#[derive(Serialize)]
struct DeriveProbe {
    #[serde(rename = "S5:v2")]
    schema: u32,
}

pub fn publish(
    clock: Res<Clock>,
    score: Res<Score>,
    arena: Res<Arena>,
    snapshot: Res<SharedSnapshot>,
    entities: Query<Entity>,
    pilot: Query<&PilotStatus, With<Captain>>,
) {
    let pilot = pilot.single().expect("exactly one captain");
    let derived = serde_json::to_string(&DeriveProbe { schema: 1 }).unwrap();
    let body = format!(
        "probe={}|S2:{}|S3:{:.0}|S4:{}|{}\nticks={}\npid={}\nscore={}\nentities={}\nwave={}\ncollected={}\ncollisions={}\ndistance={:.3}\nenergy={}\ncommands={}\nrule_activations={}\npilot_fields={}\n",
        crate::gameplay::probe_label(),
        crate::gameplay::score_reward(10),
        speed_limit(),
        reflected_fields(pilot),
        derived,
        clock.ticks,
        std::process::id(),
        score.total,
        entities.iter().count(),
        arena.wave,
        score.collected,
        score.collisions,
        score.distance,
        pilot.energy,
        score.commands,
        arena.rule_activations,
        reflected_u32_values(pilot),
    );
    *snapshot.0.write().unwrap() = body;
}
