use super::EnderDragonPhase;
use crate::entity::{
    Entity, area_effect_cloud::AreaEffectCloudEntity, boss::ender_dragon::EnderDragonEntity,
};
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::sync::{Arc, Weak};

const FLAME_DURATION: i32 = 200;
const FLAME_COUNT: i32 = 4;
const FLAME_RADIUS: f32 = 5.0;

pub struct SitBreathingPhase;

impl super::Phase for SitBreathingPhase {
    fn get_type(&self) -> EnderDragonPhase {
        EnderDragonPhase::SitBreathing
    }

    fn begin(&self, dragon: &EnderDragonEntity) {
        dragon.set_target_location(None);
        *dragon
            .breathing_timer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = 0;
        *dragon
            .flame_count
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
    }

    fn end(&self, dragon: &EnderDragonEntity) {
        let cloud = dragon
            .breathing_cloud
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(cloud) = cloud.as_ref().and_then(Weak::upgrade) {
            cloud.get_entity().remove();
        }
    }

    fn tick(&self, dragon: &EnderDragonEntity) {
        let elapsed = {
            let mut timer = dragon
                .breathing_timer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *timer += 1;
            *timer
        };
        if elapsed >= FLAME_DURATION {
            let flame_count = *dragon
                .flame_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            dragon.set_phase(if flame_count >= FLAME_COUNT {
                EnderDragonPhase::TakingOff
            } else {
                EnderDragonPhase::SitScanning
            });
        } else if elapsed == 10 {
            spawn_flame(dragon);
        }
    }
}

fn spawn_flame(dragon: &EnderDragonEntity) {
    let entity = &dragon.mob_entity.living_entity.entity;
    let pos = entity.pos.load();
    let head = &dragon.parts[0].entity;
    let head_pos = head.pos.load();
    let world = entity.world.load();
    let away = Vector3::new(head_pos.x - pos.x, 0.0, head_pos.z - pos.z).normalize();
    let ahead = head_pos + away * (f64::from(FLAME_RADIUS) / 2.0);
    let initial_y = (head_pos.y + f64::from(head.height()) / 2.0).floor() as i32;
    let x = ahead.x.floor() as i32;
    let z = ahead.z.floor() as i32;
    let y = (world.min_y..=initial_y)
        .rev()
        .find(|y| !world.get_block_state(&BlockPos::new(x, *y, z)).is_air())
        .unwrap_or(initial_y);
    let cloud_entity = Entity::new(
        world.clone(),
        Vector3::new(ahead.x, f64::from(y + 1), ahead.z),
        &EntityType::AREA_EFFECT_CLOUD,
    );
    let cloud = AreaEffectCloudEntity::create(
        cloud_entity,
        ItemStack::new(0, &Item::DRAGON_BREATH),
        vec![(&StatusEffect::INSTANT_DAMAGE, 1, 0, false, true, true)],
        FLAME_DURATION,
        FLAME_RADIUS,
        20,
        10,
        0.0,
        0,
    );
    if let Some(cloud_entity) = cloud.cast_any().downcast_ref::<AreaEffectCloudEntity>() {
        *cloud_entity
            .radius_on_tick
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = 0.0;
    }
    *dragon
        .breathing_cloud
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&cloud));
    world.spawn_entity(cloud);
}
