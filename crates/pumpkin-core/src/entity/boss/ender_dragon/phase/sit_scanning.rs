use super::EnderDragonPhase;
use crate::entity::{EntityBase, boss::ender_dragon::EnderDragonEntity};
use pumpkin_util::math::vector3::Vector3;

const SITTING_SCANNING_IDLE_TICKS: i32 = 100;
const SITTING_ATTACK_Y_VIEW_RANGE: f64 = 10.0;
const SITTING_ATTACK_VIEW_RANGE: f64 = 20.0;
const SITTING_CHARGE_VIEW_RANGE: f64 = 150.0;

pub struct SitScanningPhase;

impl super::Phase for SitScanningPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::SitScanning
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.set_target_location(None);
        *dragon
            .ticks_sitting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        let elapsed = {
            let mut ticks = dragon
                .ticks_sitting
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *ticks += 1;
            *ticks
        };
        let entity = &dragon.mob_entity.living_entity.entity;
        let pos = entity.pos.load();
        if let Some(player) = dragon.nearest_combat_player(
            pos,
            SITTING_ATTACK_VIEW_RANGE,
            SITTING_ATTACK_Y_VIEW_RANGE,
            true,
        ) {
            if elapsed > 25 {
                dragon.set_phase(EnderDragonPhase::SitAttacking);
            } else {
                face_player(dragon, player.get_entity().pos.load());
            }
        } else if elapsed >= SITTING_SCANNING_IDLE_TICKS {
            dragon.set_phase(EnderDragonPhase::TakingOff);
            if let Some(player) =
                dragon.nearest_combat_player(pos, SITTING_CHARGE_VIEW_RANGE, f64::INFINITY, true)
            {
                dragon.set_phase(EnderDragonPhase::Charging);
                *dragon
                    .charging_target
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(player.get_entity().pos.load());
            }
        }
    }
}

fn face_player(dragon: &EnderDragonEntity, target: Vector3<f64>) {
    let entity = &dragon.mob_entity.living_entity.entity;
    let pos = entity.pos.load();
    let yaw = entity.yaw.load();
    let yaw_rad = f64::from(yaw).to_radians();
    let heading = Vector3::new(yaw_rad.sin(), 0.0, -yaw_rad.cos());
    let aim = Vector3::new(target.x - pos.x, 0.0, target.z - pos.z).normalize();
    if heading.dot(&aim).clamp(-1.0, 1.0).acos().to_degrees() + 0.5 <= 10.0 {
        return;
    }
    let head = dragon.parts[0].entity.pos.load();
    let dx = target.x - head.x;
    let dz = target.z - head.z;
    let delta = (180.0 - dx.atan2(dz).to_degrees() as f32 - yaw + 180.0).rem_euclid(360.0) - 180.0;
    let distance = dx.hypot(dz) as f32 + 1.0;
    let mut yaw_acceleration = dragon
        .yaw_rot_accel
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *yaw_acceleration = *yaw_acceleration * 0.8
        + delta.clamp(-100.0, 100.0) * (0.7 / distance.min(40.0) / distance);
    entity.yaw.store(yaw + *yaw_acceleration);
}
