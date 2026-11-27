use super::EnderDragonPhase;
use crate::entity::boss::ender_dragon::{EnderDragonEntity, Vector3Ext};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use std::sync::atomic::Ordering;

pub struct DyingPhase;

impl super::Phase for DyingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::Dying
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.set_target_location(None);
    }

    fn get_fly_speed(&self) -> f32 {
        3.0
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        let living = &dragon.mob_entity.living_entity;
        let entity = &living.entity;
        let target = {
            let mut target = dragon
                .target_location
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *target.get_or_insert_with(|| {
                let origin = dragon.fight_origin_pos().0;
                let world = entity.world.load();
                let top = world.get_heightmap_height(
                    ChunkHeightmapType::MotionBlocking,
                    origin.x,
                    origin.z,
                );
                Vector3::new(
                    f64::from(origin.x) + 0.5,
                    f64::from(top + 1),
                    f64::from(origin.z) + 0.5,
                )
            })
        };
        let distance = entity.pos.load().distance_squared(target);
        if !(100.0..=22500.0).contains(&distance)
            || entity.horizontal_collision.load(Ordering::Relaxed)
            || entity.vertical_collision.load(Ordering::Relaxed)
        {
            living.set_health(0.0);
        } else {
            living.set_health(1.0);
        }
    }
}
