use crate::entity::projectile::owner::ProjectileOwner;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crossbeam::atomic::AtomicCell;

use crate::entity::experience_orb::ExperienceOrbEntity;
use crate::entity::item::ItemEntity;
use crate::entity::projectile::{
    ProjectileHit, get_hit_result_on_move_vector, hit_target_or_deflect_self, update_rotation,
};
use crate::world::World;
use crate::world::loot::{LootContextParameters, LootTableHandle, generate_loot_from_handle};
use crate::{
    entity::{Entity, EntityBase, living::LivingEntity, player::Player},
    server::Server,
};
use pumpkin_data::Block;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::fluid::Fluid;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::particle::Particle;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::Taggable;
use pumpkin_protocol::bedrock::server::ActorEventID;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::random::{RandomImpl, legacy_rand::LegacyRand};
use pumpkin_world::clip::ClipBlock;
use rand::RngExt;

#[derive(Clone, Copy, PartialEq, Eq)]
enum HookState {
    Flying,
    HookedIn,
    Bobbing,
}

pub struct FishingBobberEntity {
    pub entity: Entity,
    pub owner: ProjectileOwner,
    pub hooked_entity_id: AtomicI32,
    state: AtomicCell<HookState>,
    life: AtomicI32,
    pub wait_countdown: AtomicI32,
    pub bite_countdown: AtomicI32,
    hook_countdown: AtomicI32,
    fish_angle: AtomicCell<f32>,
    wait_time_reduction: i32,
}

impl FishingBobberEntity {
    const INERTIA: f64 = 0.92;
    const GRAVITY: f64 = 0.03;
    const MAX_GROUND_LIFE: i32 = 1200;
    const MAX_OWNER_DISTANCE_SQUARED: f64 = 1024.0;

    pub fn new(entity: Entity, owner: &Player) -> Self {
        Self::new_with_rod(entity, owner, 0)
    }

    pub fn new_with_rod(entity: Entity, owner: &Player, time_reduction: i32) -> Self {
        // Java's FishingHook.recreateFromPacket discards a hook without a valid owner id.
        entity
            .data
            .store(owner.get_entity().entity_id, Ordering::Relaxed);

        Self {
            entity,
            owner: ProjectileOwner::new(Some(&owner.living_entity.entity)),
            hooked_entity_id: AtomicI32::new(0),
            state: AtomicCell::new(HookState::Flying),
            life: AtomicI32::new(0),
            wait_countdown: AtomicI32::new(0),
            bite_countdown: AtomicI32::new(0),
            hook_countdown: AtomicI32::new(0),
            fish_angle: AtomicCell::new(0.0),
            wait_time_reduction: time_reduction.max(0),
        }
    }

    pub fn reel_in(&self, player: &Player, rod: &ItemStack) -> i32 {
        let world = self.entity.world.load();
        let hooked_id = self.hooked_entity_id.load(Ordering::Relaxed);
        let mut dmg = 0;

        if hooked_id != 0
            && let Some(hooked) = world.get_entity_by_id(hooked_id)
        {
            let player_pos = player.get_entity().pos.load();
            let bobber_pos = self.entity.pos.load();
            let delta = player_pos - bobber_pos;
            let motion =
                delta
                    .multiply(0.1, 0.1, 0.1)
                    .add_raw(0.0, delta.length().sqrt() * 0.08, 0.0);
            hooked.get_entity().add_velocity(motion);
            world.send_entity_status(
                &self.entity,
                EntityStatus::FishingRodReelIn,
                ActorEventID::FishhookHookTime,
            );
            dmg = if hooked.get_entity().entity_type == &EntityType::ITEM {
                3
            } else {
                5
            };
        } else if self.bite_countdown.load(Ordering::Relaxed) > 0 {
            let luck =
                crate::enchantment::helper::EnchantmentHelper::modify_fishing_luck_bonus(rod, 0.0)
                    + player.living_entity.get_attribute_value(&Attributes::LUCK) as f32;
            let bobber_pos = self.entity.pos.load();
            let player_pos = player.get_entity().pos.load();
            let params = LootContextParameters {
                position: Some(bobber_pos),
                tool: Some(rod.clone()),
                this_entity: Some(self),
                world: Some(&world),
                luck,
                ..Default::default()
            };
            let handle = world
                .server
                .upgrade()
                .and_then(|s| {
                    s.datapack_manager
                        .get_loot_table("minecraft:gameplay/fishing")
                })
                .unwrap_or(LootTableHandle::Static(
                    &pumpkin_data::loot_table::GAMEPLAY_FISHING,
                ));
            let items = generate_loot_from_handle(&handle, 0, &params);
            let delta = player_pos - bobber_pos;
            let vy = delta.y * 0.1 + delta.length().sqrt() * 0.08;

            for item_stack in items {
                if item_stack.is_empty() {
                    continue;
                }
                if item_stack
                    .item
                    .has_tag(&pumpkin_data::tag::Item::MINECRAFT_FISHES)
                {
                    player.increment_stat(
                        pumpkin_data::statistic::StatisticCategory::Custom,
                        pumpkin_data::statistic::CustomStatistic::FishCaught as i32,
                        1,
                    );
                }
                player.trigger_advancement(
                    crate::entity::player::advancement::trigger::AdvancementTrigger::FishedItem {
                        item_id: format!("minecraft:{}", item_stack.item.registry_key),
                    },
                );
                let item_entity = ItemEntity::new_with_velocity(
                    Entity::new(world.clone(), bobber_pos, &EntityType::ITEM),
                    item_stack,
                    Vector3::new(delta.x * 0.1, vy, delta.z * 0.1),
                    0,
                );
                world.spawn_entity(Arc::new(item_entity));
            }

            let xp_amount = rand::rng().random_range(1..=6);
            ExperienceOrbEntity::spawn(&world, player_pos.add_raw(0.0, 0.5, 0.5), xp_amount);
            dmg = 1;
        }

        if self.entity.on_ground.load(Ordering::Relaxed) {
            dmg = dmg.max(2);
        }

        dmg
    }

    #[expect(clippy::too_many_lines)]
    pub fn process_tick(&self, caller: &dyn EntityBase, server: &Server) {
        let entity = self.get_entity();
        entity.tick(caller, server);
        if entity.is_removed() {
            return;
        }
        let world = entity.world.load();
        self.owner.tick(entity);

        let Some(owner) = self.owner.get(&world) else {
            entity.remove();
            return;
        };
        let Some(player) = owner.get_player() else {
            entity.remove();
            return;
        };
        if self.should_stop_fishing(player) {
            self.discard(player);
            return;
        }

        if entity.on_ground.load(Ordering::Relaxed) {
            if self.life.fetch_add(1, Ordering::Relaxed) + 1 >= Self::MAX_GROUND_LIFE {
                self.discard(player);
                return;
            }
        } else {
            self.life.store(0, Ordering::Relaxed);
        }

        let block_pos = entity.block_pos.load();
        let (fluid, fluid_state) = world.get_fluid_and_fluid_state(&block_pos);
        let water_height = if fluid.matches_type(&Fluid::WATER) {
            f64::from(world.get_fluid_height(&block_pos, fluid, &fluid_state))
        } else {
            0.0
        };
        let in_water = water_height > 0.0;
        let mut velocity = entity.velocity.load();

        match self.state.load() {
            HookState::Flying => {
                if self.hooked_entity_id.load(Ordering::Relaxed) != 0 {
                    entity.velocity.store(Vector3::default());
                    self.state.store(HookState::HookedIn);
                    return;
                }
                if in_water {
                    entity.velocity.store(velocity.multiply(0.3, 0.2, 0.3));
                    self.state.store(HookState::Bobbing);
                    return;
                }
                let hit = get_hit_result_on_move_vector(
                    caller,
                    |other| {
                        (other.can_be_hit_by_projectile()
                            && self.owner.can_hit(&world, other.get_entity()))
                            || (other.get_entity().is_alive()
                                && other.get_entity().entity_type == &EntityType::ITEM)
                    },
                    ClipBlock::Collider,
                );
                if let Some(hit) = hit {
                    hit_target_or_deflect_self(caller, hit, server);
                }
                velocity = entity.velocity.load();
            }
            HookState::HookedIn => {
                let hooked_id = self.hooked_entity_id.load(Ordering::Relaxed);
                if let Some(hooked) = world.get_entity_by_id(hooked_id)
                    && hooked.can_interact_with_level()
                    && Arc::ptr_eq(&hooked.get_entity().world.load(), &world)
                {
                    let mut hooked_pos = hooked.get_entity().pos.load();
                    hooked_pos.y += f64::from(hooked.get_entity().height()) * 0.8;
                    entity.set_pos(hooked_pos);
                } else {
                    self.set_hooked_entity(None);
                }
                return;
            }
            HookState::Bobbing => {
                velocity = bob_velocity(
                    velocity,
                    entity.pos.load().y,
                    block_pos.0.y,
                    water_height,
                    rand::random::<f32>(),
                );
                if in_water {
                    if self.bite_countdown.load(Ordering::Relaxed) > 0 {
                        let mut synced = self.synced_random(&world);
                        velocity.y -= 0.1 * f64::from(synced.next_f32() * synced.next_f32());
                    }
                    self.catching_fish(&world, &block_pos, &mut velocity);
                }
            }
        }

        if !in_water
            && !entity.on_ground.load(Ordering::Relaxed)
            && self.hooked_entity_id.load(Ordering::Relaxed) == 0
        {
            velocity.y -= Self::GRAVITY;
        }
        entity.velocity.store(velocity);
        entity.move_entity(caller, velocity);
        update_rotation(entity, false);
        if self.state.load() == HookState::Flying
            && (entity.on_ground.load(Ordering::Relaxed)
                || entity.horizontal_collision.load(Ordering::Relaxed))
        {
            entity.velocity.store(Vector3::default());
        }
        entity
            .velocity
            .store(entity.velocity.load() * Self::INERTIA);
    }

    fn should_stop_fishing(&self, player: &Player) -> bool {
        let world = self.entity.world.load();
        if !player.can_interact_with_level() || !Arc::ptr_eq(&player.world(), &world) {
            return true;
        }
        let inventory = player.inventory();
        let holding_rod = inventory.held_item().item.id == Item::FISHING_ROD.id
            || inventory
                .get_stack_in_hand(pumpkin_util::Hand::Left)
                .item
                .id
                == Item::FISHING_ROD.id;
        !holding_rod
            || player
                .position()
                .squared_distance_to_vec(&self.entity.pos.load())
                > Self::MAX_OWNER_DISTANCE_SQUARED
    }

    fn discard(&self, player: &Player) {
        let _ = player.fishing_bobber.compare_exchange(
            self.entity.entity_id,
            -1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        self.entity.remove();
    }

    fn synced_random(&self, world: &World) -> LegacyRand {
        let (_, uuid_low) = self.entity.entity_uuid.as_u64_pair();
        let game_time = world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .world_age;
        LegacyRand::from_seed(uuid_low ^ game_time as u64)
    }

    /// Matches vanilla `FishingBobberEntity.catchingFish(BlockPos)`.
    #[expect(clippy::too_many_lines)]
    fn catching_fish(&self, world: &World, block_pos: &BlockPos, velocity: &mut Vector3<f64>) {
        let entity = self.get_entity();
        let pos = entity.pos.load();

        let mut fishing_speed = 1;
        let above = block_pos.up();
        if rand::random::<f32>() < 0.25 && world.is_raining_at(&above) {
            fishing_speed += 1;
        }
        if rand::random::<f32>() < 0.5 && !world.can_see_sky(&above) {
            fishing_speed -= 1;
        }

        let nibble = self.bite_countdown.load(Ordering::Relaxed);
        if nibble > 0 {
            let next = nibble - 1;
            self.bite_countdown.store(next, Ordering::Relaxed);
            if next <= 0 {
                self.wait_countdown.store(0, Ordering::Relaxed);
                self.hook_countdown.store(0, Ordering::Relaxed);
                entity.set_synced_data(
                    pumpkin_data::tracked_data::fishing_bobber::DATA_BITING,
                    false,
                );
            }
        } else if self.hook_countdown.load(Ordering::Relaxed) > 0 {
            let hooked = self.hook_countdown.load(Ordering::Relaxed) - fishing_speed;
            self.hook_countdown.store(hooked, Ordering::Relaxed);
            if hooked > 0 {
                // Vanilla `RandomSource.triangle(0.0, 9.188)`: the approaching fish wanders.
                let angle = self.fish_angle.load()
                    + (9.188 * (rand::random::<f64>() - rand::random::<f64>())) as f32;
                self.fish_angle.store(angle);
                let radians = f64::from(angle).to_radians();
                let angle_sin = radians.sin();
                let angle_cos = radians.cos();
                let fish_x = pos.x + angle_sin * f64::from(hooked) * 0.1;
                let fish_y = f64::from(block_pos.0.y) + 1.0;
                let fish_z = pos.z + angle_cos * f64::from(hooked) * 0.1;
                let splash_pos = BlockPos::floored(fish_x, fish_y - 1.0, fish_z);
                if world.get_block(&splash_pos) == &Block::WATER {
                    if rand::random::<f32>() < 0.15 {
                        world.spawn_particle(
                            Vector3::new(fish_x, fish_y - 0.1, fish_z),
                            Vector3::new(angle_sin as f32, 0.1, angle_cos as f32),
                            0.0,
                            1,
                            Particle::Bubble,
                        );
                    }

                    let x_movement = angle_sin as f32 * 0.04;
                    let z_movement = angle_cos as f32 * 0.04;
                    world.spawn_particle(
                        Vector3::new(fish_x, fish_y, fish_z),
                        Vector3::new(z_movement, 0.01, -x_movement),
                        1.0,
                        0,
                        Particle::Fishing,
                    );
                    world.spawn_particle(
                        Vector3::new(fish_x, fish_y, fish_z),
                        Vector3::new(-z_movement, 0.01, x_movement),
                        1.0,
                        0,
                        Particle::Fishing,
                    );
                }
            } else {
                world.play_sound_fine(
                    Sound::EntityFishingBobberSplash,
                    SoundCategory::Neutral,
                    &pos,
                    0.25,
                    1.0 + (rand::random::<f32>() - rand::random::<f32>()) * 0.4,
                );

                let width = entity.width();
                let count = (1.0 + width * 20.0) as i32;
                let particle_pos = Vector3::new(pos.x, pos.y + 0.5, pos.z);
                let particle_offset = Vector3::new(width, 0.0, width);
                world.spawn_particle(particle_pos, particle_offset, 0.2, count, Particle::Bubble);
                world.spawn_particle(particle_pos, particle_offset, 0.2, count, Particle::Fishing);

                self.bite_countdown
                    .store(rand::random_range(20..=40), Ordering::Relaxed);
                entity.set_synced_data(
                    pumpkin_data::tracked_data::fishing_bobber::DATA_BITING,
                    true,
                );
                // Vanilla applies the bite's downward kick in `onSyncedDataUpdated`.
                let mut synced = self.synced_random(world);
                velocity.y = -0.4 * (f64::from(synced.next_f32()) * 0.4 + 0.6);
            }
        } else if self.wait_countdown.load(Ordering::Relaxed) > 0 {
            let wait = self.wait_countdown.load(Ordering::Relaxed) - fishing_speed;
            self.wait_countdown.store(wait, Ordering::Relaxed);
            let mut tease_chance = 0.15;
            if wait < 20 {
                tease_chance += f64::from(20 - wait) * 0.05;
            } else if wait < 40 {
                tease_chance += f64::from(40 - wait) * 0.02;
            } else if wait < 60 {
                tease_chance += f64::from(60 - wait) * 0.01;
            }

            if rand::random::<f64>() < tease_chance {
                let angle = f64::from(rand::random::<f32>() * 360.0).to_radians();
                let distance = 25.0 + rand::random::<f64>() * 35.0;
                let fish_x = pos.x + angle.sin() * distance * 0.1;
                let fish_y = f64::from(block_pos.0.y) + 1.0;
                let fish_z = pos.z + angle.cos() * distance * 0.1;
                let splash_pos = BlockPos::floored(fish_x, fish_y - 1.0, fish_z);
                if world.get_block(&splash_pos) == &Block::WATER {
                    world.spawn_particle(
                        Vector3::new(fish_x, fish_y, fish_z),
                        Vector3::new(0.1, 0.0, 0.1),
                        0.0,
                        2 + rand::random_range(0..2),
                        Particle::Splash,
                    );
                }
            }

            if wait <= 0 {
                self.fish_angle.store(rand::random::<f32>() * 360.0);
                self.hook_countdown
                    .store(rand::random_range(20..=80), Ordering::Relaxed);
            }
        } else {
            let wait = rand::random_range(100..=600) - self.wait_time_reduction;
            self.wait_countdown.store(wait, Ordering::Relaxed);
        }
    }

    fn set_hooked_entity(&self, hooked: Option<&Entity>) {
        self.hooked_entity_id.store(
            hooked.map_or(0, |entity| entity.entity_id),
            Ordering::Relaxed,
        );
        if hooked.is_none() {
            self.state.store(HookState::Flying);
        }
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::fishing_bobber::HOOKED_ENTITY,
            hooked.map_or(0, |entity| entity.entity_id + 1),
        );
    }
}

/// Vanilla FishingHook's spring toward the current water surface.
fn bob_velocity(
    velocity: Vector3<f64>,
    y: f64,
    block_y: i32,
    water_height: f64,
    random: f32,
) -> Vector3<f64> {
    let mut force = y + velocity.y - f64::from(block_y) - water_height;
    if force.abs() < 0.01 {
        // Java Math.signum(0.0) is zero, unlike Rust's float signum.
        force += if force > 0.0 {
            0.1
        } else if force < 0.0 {
            -0.1
        } else {
            0.0
        };
    }
    Vector3::new(
        velocity.x * 0.9,
        velocity.y - force * f64::from(random) * 0.2,
        velocity.z * 0.9,
    )
}

impl EntityBase for FishingBobberEntity {
    fn should_bounce_on_world_border(&self) -> bool {
        true
    }

    fn get_projectile_owner(&self) -> Option<&ProjectileOwner> {
        Some(&self.owner)
    }

    fn entity_bedrock_spawn_metadata(
        &self,
    ) -> Option<pumpkin_protocol::bedrock::client::set_actor_data::SyncedActorDataList> {
        let mut meta =
            pumpkin_protocol::bedrock::client::set_actor_data::SyncedActorDataList::new();
        meta.set(
            pumpkin_protocol::bedrock::client::set_actor_data::entity_data_key::OWNER,
            pumpkin_protocol::bedrock::client::set_actor_data::MetadataValue::Int64(i64::from(
                self.owner
                    .get(&self.entity.world.load())
                    .map_or(0, |owner| owner.get_entity().entity_id),
            )),
        );
        Some(meta)
    }

    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
    fn on_hit(&self, hit: ProjectileHit) {
        match hit {
            ProjectileHit::Entity { entity, .. } => {
                self.set_hooked_entity(Some(entity.get_entity()))
            }
            ProjectileHit::Block { hit_pos, .. } => {
                let entity = &self.entity;
                let distance = (hit_pos - entity.pos.load()).length();
                let velocity = entity.velocity.load().normalize() * distance;
                entity.velocity.store(velocity);
            }
        }
    }

    fn tick(&self, caller: &dyn EntityBase, server: &Server) {
        self.process_tick(caller, server);
    }
}

#[cfg(test)]
mod tests {
    use super::bob_velocity;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn bobber_surface_equilibrium_does_not_drift() {
        let still = Vector3::default();
        assert_eq!(bob_velocity(still, 65.0, 64, 1.0, 0.5), still);
        assert!(bob_velocity(still, 64.9, 64, 1.0, 0.5).y > 0.0);
        assert!(bob_velocity(still, 65.1, 64, 1.0, 0.5).y < 0.0);
    }
}
