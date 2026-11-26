use pumpkin_data::damage::DamageType;
use pumpkin_data::entity::EntityType;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::GameMode;
use pumpkin_util::math::boundingbox::EntityDimensions;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::{chunk::ChunkHeightmapType, world::BlockFlags};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};

use crate::entity::{
    Entity, EntityBase,
    living::LivingEntity,
    mob::{Mob, MobEntity},
    player::Player,
};

pub mod flight_history;
pub mod phase;

use flight_history::DragonFlightHistory;
use phase::{EnderDragonPhase, PhaseManager};

pub const NODE_COUNT: usize = 24;
pub const NODE_Y: i32 = 105;
pub const NODE_REACH_SQ: f64 = 64.0;
pub const DEATH_TIMER_MAX: i32 = 200;
const GROWL_COOLDOWN_BASE: i32 = 200;
const FLAP_SPEED: f32 = 0.2;
pub const FLY_ACCEL: f64 = 0.1;
pub const FLY_SPEED: f64 = 0.6;
const DRAGON_PART_DIMENSIONS: [(f32, f32); 8] = [
    (1.0, 1.0), // Head
    (3.0, 3.0), // Neck
    (5.0, 3.0), // Body
    (2.0, 2.0), // Tail
    (2.0, 2.0),
    (2.0, 2.0),
    (4.0, 2.0), // Wings
    (4.0, 2.0),
];

const NODE_ADJACENCY: [u32; NODE_COUNT] = [
    6146, 8197, 8202, 16404, 32808, 32848, 65696, 131392, 131712, 263424, 526848, 525313, 1581057,
    3166214, 2138120, 6373424, 4358208, 12910976, 9044480, 9706496, 15216640, 13688832, 11763712,
    8257536,
];

#[derive(Clone, Copy, Default, Debug)]
pub struct DragonNode {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl DragonNode {
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[inline]
    #[must_use]
    pub fn dist_sq(&self, other: &Self) -> f64 {
        let (dx, dy, dz) = (self.x - other.x, self.y - other.y, self.z - other.z);
        dx * dx + dy * dy + dz * dz
    }

    #[inline]
    #[must_use]
    pub fn dist_sq_vec(&self, v: Vector3<f64>) -> f64 {
        let (dx, dy, dz) = (self.x - v.x, self.y - v.y, self.z - v.z);
        dx * dx + dy * dy + dz * dz
    }

    #[inline]
    #[must_use]
    pub const fn as_vec3(&self) -> Vector3<f64> {
        Vector3::new(self.x, self.y, self.z)
    }
}

#[derive(PartialEq, Clone, Copy)]
struct OrderedFloat(f64);

impl Eq for OrderedFloat {}

impl PartialOrd for OrderedFloat {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedFloat {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[must_use]
pub fn find_path(
    nodes: &[Option<DragonNode>; NODE_COUNT],
    from: usize,
    to: usize,
    first_node: usize,
) -> Vec<usize> {
    if from == to {
        return Vec::new();
    }

    let mut g = [f64::MAX; NODE_COUNT];
    let mut came_from = [NODE_COUNT; NODE_COUNT];
    let mut closed = [false; NODE_COUNT];
    let mut open: std::collections::BinaryHeap<(OrderedFloat, usize)> =
        std::collections::BinaryHeap::new();

    let h = |n: usize| -> f64 {
        match (nodes[n], nodes[to]) {
            (Some(a), Some(b)) => a.dist_sq(&b).sqrt(),
            _ => 0.0,
        }
    };

    g[from] = 0.0;
    open.push((OrderedFloat(-h(from)), from));
    let mut closest = from;

    while let Some((_, cur)) = open.pop() {
        if closed[cur] {
            continue;
        }
        if cur == to {
            closest = cur;
            break;
        }
        if h(cur) < h(closest) {
            closest = cur;
        }
        closed[cur] = true;

        let adj = NODE_ADJACENCY[cur];
        for nbr in first_node..NODE_COUNT {
            if (adj >> nbr) & 1 == 0 || closed[nbr] {
                continue;
            }
            let (Some(cn), Some(nn)) = (nodes[cur], nodes[nbr]) else {
                continue;
            };
            let ng = g[cur] + cn.dist_sq(&nn).sqrt();
            if ng < g[nbr] {
                g[nbr] = ng;
                came_from[nbr] = cur;
                open.push((OrderedFloat(-(ng + h(nbr))), nbr));
            }
        }
    }

    let mut path = Vec::new();
    let mut cur = closest;
    while cur != NODE_COUNT {
        path.push(cur);
        cur = came_from[cur];
    }
    path.reverse();
    if path.first() == Some(&from) {
        path.remove(0);
    }
    path
}

pub struct EnderDragonPart {
    pub entity: Entity,
    pub dragon_uuid: uuid::Uuid,
    dragon_id: i32,
    takes_full_damage: bool,
}

impl EnderDragonPart {
    fn new(
        entity: Entity,
        dragon_uuid: uuid::Uuid,
        dragon_id: i32,
        takes_full_damage: bool,
        dimensions: EntityDimensions,
    ) -> Self {
        entity.entity_dimension.store(dimensions);
        entity.bounding_box.store(Entity::make_bounding_box(
            entity.entity_type,
            entity.pos.load(),
            &dimensions,
        ));
        Self {
            entity,
            dragon_uuid,
            dragon_id,
            takes_full_damage,
        }
    }
}

impl EntityBase for EnderDragonPart {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        amount: f32,
        damage_type: DamageType,
        position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let attacker = cause.or(source);
        let world = attacker.map_or_else(
            || self.entity.world.load_full(),
            |attacker| attacker.get_entity().world.load_full(),
        );
        let Some(parent) = world.get_entity_by_id(self.dragon_id) else {
            return false;
        };
        let Some(dragon) = parent.cast_any().downcast_ref::<EnderDragonEntity>() else {
            return false;
        };
        let living = &dragon.mob_entity.living_entity;
        if self.entity.is_removed()
            || living.entity.is_removed()
            || !Arc::ptr_eq(&world, &living.entity.world.load())
            || !dragon
                .parts
                .iter()
                .any(|part| std::ptr::eq(part.as_ref(), self))
            || *dragon
                .phase
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                == EnderDragonPhase::Dying
        {
            return false;
        }
        let amount = if self.takes_full_damage {
            amount
        } else {
            amount / 4.0 + amount.min(1.0)
        };
        if amount < 0.01
            || (attacker.and_then(EntityBase::get_player).is_none()
                && !damage_type.has_tag(&tag::DamageType::MINECRAFT_ALWAYS_HURTS_ENDER_DRAGONS))
        {
            return false;
        }
        let health_before = living.health.load();
        let applied = parent.damage_with_context(
            parent.as_ref(),
            amount,
            damage_type,
            position,
            source,
            cause,
        );
        if applied {
            dragon.hurt((health_before - living.health.load()).max(0.0));
        }
        applied
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }
    fn can_hit(&self) -> bool {
        true
    }
}

pub struct EnderDragonEntity {
    pub mob_entity: MobEntity,
    pub parts: Vec<Arc<EnderDragonPart>>,

    pub flight_history: Mutex<DragonFlightHistory>,
    pub flap_time: Mutex<f32>,
    pub o_flap_time: Mutex<f32>,

    pub in_wall: Mutex<bool>,
    pub dragon_death_time: Mutex<i32>,
    pub sitting_damage_received: Mutex<f32>,

    pub nodes_initialized: Mutex<bool>,

    pub fight_origin: Mutex<BlockPos>,

    pub phase_manager: PhaseManager,
    pub phase: Mutex<EnderDragonPhase>,
    pub growl_time: Mutex<i32>,
    pub yaw_rot_accel: Mutex<f32>,
    pub holding_pattern_clockwise: Mutex<bool>,
    pub target_location: Mutex<Option<Vector3<f64>>>,
    pub fireball_charge: Mutex<i32>,

    pub nodes: Mutex<[Option<DragonNode>; NODE_COUNT]>,
    pub path: Mutex<Vec<usize>>,

    pub strafe_target: Mutex<Option<Vector3<f64>>>,
    pub target_player: Mutex<Option<uuid::Uuid>>,
    pub ticks_sitting: Mutex<i32>,
    pub sit_attack_timer: Mutex<i32>,
    pub breathing_timer: Mutex<i32>,
    pub(super) flame_count: Mutex<i32>,
    pub(super) breathing_cloud: Mutex<Option<Weak<dyn EntityBase>>>,
    pub(super) charging_target: Mutex<Option<Vector3<f64>>>,
}

impl EnderDragonEntity {
    pub(crate) const PART_COUNT: i32 = DRAGON_PART_DIMENSIONS.len() as i32;

    pub fn new(entity: Entity) -> Arc<Self> {
        entity.no_physics.store(true, Ordering::Relaxed);
        let base_id = entity.entity_id;
        let world = entity.world.load();

        let parts = (1..=Self::PART_COUNT)
            .zip(DRAGON_PART_DIMENSIONS)
            .map(|(offset, (width, height))| {
                let part = Entity::from_uuid_with_id(
                    base_id.wrapping_add(offset),
                    uuid::Uuid::new_v4(),
                    world.clone(),
                    entity.pos.load(),
                    &EntityType::ENDER_DRAGON,
                );
                Arc::new(EnderDragonPart::new(
                    part,
                    entity.entity_uuid,
                    base_id,
                    offset <= 2,
                    EntityDimensions::new(width, height, 0.0),
                ))
            })
            .collect();

        Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            parts,
            flight_history: Mutex::new(DragonFlightHistory::default()),
            flap_time: Mutex::new(0.0),
            o_flap_time: Mutex::new(0.0),
            in_wall: Mutex::new(false),
            dragon_death_time: Mutex::new(0),
            sitting_damage_received: Mutex::new(0.0),
            nodes_initialized: Mutex::new(false),
            fight_origin: Mutex::new(BlockPos::new(0, 128, 0)),
            phase_manager: PhaseManager::new(),
            phase: Mutex::new(EnderDragonPhase::Circling),
            growl_time: Mutex::new(GROWL_COOLDOWN_BASE),
            yaw_rot_accel: Mutex::new(0.0),
            holding_pattern_clockwise: Mutex::new(true),
            target_location: Mutex::new(None),
            fireball_charge: Mutex::new(0),
            nodes: Mutex::new([None; NODE_COUNT]),
            path: Mutex::new(Vec::new()),
            strafe_target: Mutex::new(None),
            target_player: Mutex::new(None),
            ticks_sitting: Mutex::new(0),
            sit_attack_timer: Mutex::new(0),
            breathing_timer: Mutex::new(0),
            flame_count: Mutex::new(0),
            breathing_cloud: Mutex::new(None),
            charging_target: Mutex::new(None),
        })
    }

    pub fn set_fight_origin(&self, pos: BlockPos) {
        if let (Ok(mut initialized), Ok(mut origin)) = (
            self.nodes_initialized.try_lock(),
            self.fight_origin.try_lock(),
        ) && *origin != pos
        {
            *origin = pos;
            *initialized = false;
        }
    }

    pub fn set_phase(&self, phase_type: EnderDragonPhase) {
        let mut phase_lock = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *phase_lock == phase_type {
            return;
        }

        let old_phase = self.phase_manager.get_phase(*phase_lock);
        old_phase.end(self);

        *phase_lock = phase_type;
        self.sync_phase(phase_type);

        let new_phase = self.phase_manager.get_phase(phase_type);
        new_phase.begin(self);
    }

    fn sync_phase(&self, phase: EnderDragonPhase) {
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::ender_dragon::DATA_PHASE,
            VarInt(phase.network_id()),
        );
    }

    pub(super) fn current_phase(&self) -> EnderDragonPhase {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn podium_position(&self) -> Vector3<f64> {
        let origin = self.fight_origin_pos();
        let world = self.mob_entity.living_entity.entity.world.load();
        let top = world.get_heightmap_height(
            ChunkHeightmapType::MotionBlockingNoLeaves,
            origin.0.x,
            origin.0.z,
        );
        Vector3::new(
            f64::from(origin.0.x) + 0.5,
            f64::from(top + 1),
            f64::from(origin.0.z) + 0.5,
        )
    }

    fn fight_origin_pos(&self) -> BlockPos {
        *self
            .fight_origin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn alive_crystals(&self) -> Option<i32> {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world.dragon_fight.as_ref().and_then(|fight| {
            let fight = fight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (fight.dragon_uuid() == Some(entity.entity_uuid)).then(|| fight.alive_crystals())
        })
    }

    fn first_flight_node(&self) -> usize {
        if self.alive_crystals().unwrap_or(0) > 0 {
            0
        } else {
            12
        }
    }

    fn set_target_location(&self, target: Option<Vector3<f64>>) {
        *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
    }

    fn reset_flight_path(&self) {
        self.path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.set_target_location(None);
    }

    fn flight_target_reached(&self) -> bool {
        let entity = &self.mob_entity.living_entity.entity;
        let current = entity.pos.load();
        let target = *self
            .target_location
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        target.is_none_or(|target| !(100.0..=22500.0).contains(&current.distance_squared(target)))
            || entity.horizontal_collision.load(Ordering::Relaxed)
            || entity.vertical_collision.load(Ordering::Relaxed)
    }

    fn plan_flight_path(&self, from: usize, to: usize) {
        let first_node = self.first_flight_node();
        let path = find_path(
            &self
                .nodes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            from,
            to,
            first_node,
        );
        *self
            .path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = path;
    }

    fn follow_next_flight_node(&self, fallback: Option<usize>) -> bool {
        let next = {
            let mut path = self
                .path
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if path.is_empty() {
                fallback
            } else {
                Some(path.remove(0))
            }
        };
        let node = next.and_then(|idx| {
            self.nodes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)[idx]
        });
        if let Some(node) = node {
            let target = node.as_vec3() + Vector3::new(0.0, rand::random_range(0.0..20.0), 0.0);
            self.set_target_location(Some(target));
        }
        node.is_some()
    }

    fn nearest_combat_player(
        &self,
        position: Vector3<f64>,
        radius: f64,
        max_vertical_difference: f64,
        require_line_of_sight: bool,
    ) -> Option<Arc<Player>> {
        let entity = &self.mob_entity.living_entity.entity;
        let world = entity.world.load();
        world
            .players
            .load()
            .iter()
            .filter(|player| {
                let target = player.get_entity();
                let target_pos = target.pos.load();
                player.living_entity.is_alive()
                    && !matches!(
                        player.gamemode.load(),
                        GameMode::Creative | GameMode::Spectator
                    )
                    && target_pos.distance_squared(position) <= radius * radius
                    && (target_pos.y - position.y).abs() <= max_vertical_difference
                    && (!require_line_of_sight || entity.has_line_of_sight(target))
            })
            .min_by(|a, b| {
                a.get_entity()
                    .pos
                    .load()
                    .distance_squared(position)
                    .total_cmp(&b.get_entity().pos.load().distance_squared(position))
            })
            .cloned()
    }

    fn ensure_nodes_initialized(&self) {
        let mut initialized = self
            .nodes_initialized
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *initialized {
            return;
        }

        let world = self.mob_entity.living_entity.entity.world.load();
        let fight_origin = self
            .fight_origin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut nodes = self
            .nodes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for i in 0..NODE_COUNT {
            let mut y_adjustment = 5;
            let node_x;
            let node_z;

            if i < 12 {
                node_x = fight_origin.0.x
                    + (60.0 * ((i as f32) * std::f32::consts::TAU / 12.0).cos()).floor() as i32;
                node_z = fight_origin.0.z
                    + (60.0 * ((i as f32) * std::f32::consts::TAU / 12.0).sin()).floor() as i32;
            } else if i < 20 {
                let multiplier = i - 12;
                node_x = fight_origin.0.x
                    + (40.0 * ((multiplier as f32) * std::f32::consts::TAU / 8.0).cos()).floor()
                        as i32;
                node_z = fight_origin.0.z
                    + (40.0 * ((multiplier as f32) * std::f32::consts::TAU / 8.0).sin()).floor()
                        as i32;
                y_adjustment += 10;
            } else {
                let multiplier = i - 20;
                node_x = fight_origin.0.x
                    + (20.0 * ((multiplier as f32) * std::f32::consts::TAU / 4.0).cos()).floor()
                        as i32;
                node_z = fight_origin.0.z
                    + (20.0 * ((multiplier as f32) * std::f32::consts::TAU / 4.0).sin()).floor()
                        as i32;
            }

            let height = world.get_heightmap_height(
                ChunkHeightmapType::MotionBlockingNoLeaves,
                node_x,
                node_z,
            );
            let node_y = 73.max(height + 1 + y_adjustment);
            nodes[i] = Some(DragonNode::new(node_x as f64, node_y as f64, node_z as f64));
        }

        *initialized = true;
    }

    pub fn find_closest_node(&self) -> usize {
        let pos = self.mob_entity.living_entity.entity.pos.load();
        self.find_closest_node_to(pos)
    }

    pub fn find_closest_node_to(&self, pos: Vector3<f64>) -> usize {
        self.ensure_nodes_initialized();
        let first = self.first_flight_node();
        let nodes = self
            .nodes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        nodes
            .iter()
            .enumerate()
            .skip(first)
            .filter_map(|(i, n)| n.map(|n| (i, n.dist_sq_vec(pos))))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(first, |(i, _)| i)
    }

    #[inline]
    #[must_use]
    pub fn yaw_toward(from: Vector3<f64>, to: Vector3<f64>) -> f32 {
        (-(to.z - from.z)).atan2(to.x - from.x).to_degrees() as f32 - 90.0
    }

    pub fn move_relative(&self, speed: f32, relative_movement: Vector3<f64>) {
        let yaw = self.mob_entity.living_entity.entity.yaw.load();
        let movement = Self::get_relative_movement(relative_movement, speed, yaw);
        let entity = &self.mob_entity.living_entity.entity;
        let vel = entity.velocity.load();
        entity.velocity.store(vel + movement);
    }

    fn get_relative_movement(
        relative_movement: Vector3<f64>,
        speed: f32,
        yaw: f32,
    ) -> Vector3<f64> {
        let dist = relative_movement.length_squared();
        if dist < 1.0E-7 {
            return Vector3::new(0.0, 0.0, 0.0);
        }
        let vec = if dist > 1.0 {
            relative_movement.normalize()
        } else {
            relative_movement
        } * (speed as f64);
        let sin = (yaw * (std::f32::consts::PI / 180.0)).sin() as f64;
        let cos = (yaw * (std::f32::consts::PI / 180.0)).cos() as f64;
        Vector3::new(vec.x * cos - vec.z * sin, vec.y, vec.z * cos + vec.x * sin)
    }

    pub fn steer_toward(
        &self,
        pos: Vector3<f64>,
        target: Vector3<f64>,
        fly_speed: f32,
        turn_speed: f32,
    ) {
        let xdd = target.x - pos.x;
        let mut ydd = target.y - pos.y;
        let zdd = target.z - pos.z;
        let dist_sq = xdd * xdd + ydd * ydd + zdd * zdd;

        let horizontal_dist = xdd.hypot(zdd);
        if horizontal_dist > 0.0 {
            ydd = (ydd / horizontal_dist).clamp(-(fly_speed as f64), fly_speed as f64);
        }

        let entity = &self.mob_entity.living_entity.entity;
        let mut vel = entity.velocity.load();
        vel.y += ydd * 0.01;
        entity.velocity.store(vel);

        let yaw = entity.yaw.load();
        let aim_diff = target - pos;
        let aim = if aim_diff.length_squared() > 1e-6 {
            aim_diff.normalize()
        } else {
            Vector3::new(0.0, 0.0, 0.0)
        };

        let dir_vec = Vector3::new(
            (yaw * (std::f32::consts::PI / 180.0)).sin() as f64,
            vel.y,
            -(yaw * (std::f32::consts::PI / 180.0)).cos() as f64,
        );
        let dir = if dir_vec.length_squared() > 1e-6 {
            dir_vec.normalize()
        } else {
            Vector3::new(0.0, 0.0, 0.0)
        };

        let dot = (dir.dot(&aim) as f32 + 0.5).max(0.0) / 1.5;
        if xdd.abs() > 1.0E-5 || zdd.abs() > 1.0E-5 {
            let mut y_rot_d =
                (180.0 - (xdd.atan2(zdd).to_degrees() as f32) - yaw).rem_euclid(360.0);
            if y_rot_d > 180.0 {
                y_rot_d -= 360.0;
            }
            y_rot_d = y_rot_d.clamp(-50.0, 50.0);

            let mut y_rot_a = self
                .yaw_rot_accel
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *y_rot_a *= 0.8;
            *y_rot_a += y_rot_d * turn_speed;
            entity.yaw.store(yaw + *y_rot_a * 0.1);
        }

        let span = (2.0 / (dist_sq + 1.0)) as f32;
        self.move_relative(
            0.06 * (dot * span + (1.0 - span)),
            Vector3::new(0.0, 0.0, -1.0),
        );

        let actual_vel = entity.velocity.load();
        if self.current_phase() != EnderDragonPhase::Dying {
            let inside_wall = *self
                .in_wall
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let movement = if inside_wall {
                actual_vel * 0.8
            } else {
                actual_vel
            };
            entity.move_entity(self, movement);
        }
        let actual_dir = if actual_vel.length_squared() > 1e-6 {
            actual_vel.normalize()
        } else {
            Vector3::new(0.0, 0.0, 0.0)
        };
        let slide = 0.8 + 0.15 * (actual_dir.dot(&dir) + 1.0) / 2.0;
        entity.velocity.store(Vector3::new(
            actual_vel.x * slide,
            actual_vel.y * 0.91,
            actual_vel.z * slide,
        ));
    }

    fn update_flap_time(&self) {
        let sitting = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_sitting();
        let in_wall = *self
            .in_wall
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut flap = self
            .flap_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut o_flap = self
            .o_flap_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        *o_flap = *flap;
        *flap += if sitting {
            0.1
        } else if in_wall {
            FLAP_SPEED * 0.5
        } else {
            FLAP_SPEED
        };
        if *flap > std::f32::consts::TAU {
            *flap -= std::f32::consts::TAU;
        }
    }

    fn tick_growl(&self) {
        if self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_sitting()
        {
            return;
        }
        let mut t = self
            .growl_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *t > 0 {
            *t -= 1;
        } else {
            *t = GROWL_COOLDOWN_BASE + rand::random_range(0..100);
        }
    }

    fn handle_player_collisions(&self) {
        let world = self.mob_entity.living_entity.entity.world.load();

        let dragon_bbox = self.mob_entity.living_entity.entity.bounding_box.load();
        let xm = f64::midpoint(dragon_bbox.min.x, dragon_bbox.max.x);
        let zm = f64::midpoint(dragon_bbox.min.z, dragon_bbox.max.z);

        for player in world.players.load().iter() {
            if player
                .living_entity
                .entity
                .bounding_box
                .load()
                .intersects(&dragon_bbox)
            {
                let player_pos = player.get_entity().pos.load();
                let xd = player_pos.x - xm;
                let zd = player_pos.z - zm;
                let dd = (xd * xd + zd * zd).max(0.1);

                player.living_entity.entity.apply_knockback(
                    4.0,
                    xd / dd,
                    zd / dd,
                    Some(&self.mob_entity.living_entity.entity),
                );
                player.get_entity().send_velocity();

                if !self
                    .phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_sitting()
                {
                    player.damage(self, 5.0, DamageType::MOB_ATTACK);
                }
            }
        }
    }

    fn tick_crystal_healing(&self) {
        let world = self.mob_entity.living_entity.entity.world.load();
        let pos = self.mob_entity.living_entity.entity.pos.load();

        let mut nearest_crystal = None;
        let mut min_dist_sq = 1024.0; // 32 blocks

        for entity in world.entities.load().iter() {
            if entity.get_entity().entity_type == &EntityType::END_CRYSTAL {
                let crystal_pos = entity.get_entity().pos.load();
                let dist_sq = pos.distance_squared(crystal_pos);
                if dist_sq < min_dist_sq {
                    min_dist_sq = dist_sq;
                    nearest_crystal = Some(entity.clone());
                }
            }
        }

        if let Some(_crystal) = nearest_crystal {
            let living = &self.mob_entity.living_entity;
            if living.health.load() < living.get_max_health() {
                living.heal(1.0);
            }
        }
    }

    fn tick_block_breaking(&self) {
        let phase_type = *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if phase_type.is_sitting() || phase_type == EnderDragonPhase::Dying {
            return;
        }

        let world = self.mob_entity.living_entity.entity.world.load();
        let bbox = self.mob_entity.living_entity.entity.bounding_box.load();

        let min = bbox.min_block_pos();
        let max = bbox.max_block_pos();

        for pos in BlockPos::iterate(min, max) {
            let block = world.get_block(&pos);
            if block != &Block::BEDROCK
                && block != &Block::END_STONE
                && block != &Block::OBSIDIAN
                && block != &Block::IRON_BARS
                && block != &Block::END_PORTAL
                && block != &Block::END_PORTAL_FRAME
            {
                world.set_block_state(&pos, BlockStateId::AIR, BlockFlags::NOTIFY_ALL);
            }
        }
    }

    fn tick_parts(&self) {
        let history = self
            .flight_history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let p5 = history.get(5);
        let p10 = history.get(10);
        let p0 = history.get(0);

        let tilt = (p5.y - p10.y) as f32 * 10.0 * (std::f32::consts::PI / 180.0);
        let cc_tilt = tilt.cos() as f64;
        let ss_tilt = tilt.sin() as f64;

        let yaw = self.mob_entity.living_entity.entity.yaw.load();
        let rot1 = yaw * (std::f32::consts::PI / 180.0);
        let ss1 = rot1.sin() as f64;
        let cc1 = rot1.cos() as f64;

        let pos = self.mob_entity.living_entity.entity.pos.load();

        // Body
        self.parts[2]
            .entity
            .set_pos(Vector3::new(pos.x + ss1 * 0.5, pos.y, pos.z - cc1 * 0.5));

        // Wings
        self.parts[6].entity.set_pos(Vector3::new(
            pos.x + cc1 * 4.5,
            pos.y + 2.0,
            pos.z + ss1 * 4.5,
        ));
        self.parts[7].entity.set_pos(Vector3::new(
            pos.x - cc1 * 4.5,
            pos.y + 2.0,
            pos.z - ss1 * 4.5,
        ));

        let head_y_offset = if self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_sitting()
        {
            -1.0
        } else {
            p5.y - p0.y
        };

        let yaw_accel = *self
            .yaw_rot_accel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rot2 = (yaw - yaw_accel * 0.01) * (std::f32::consts::PI / 180.0);
        let ss2 = rot2.sin() as f64;
        let cc2 = rot2.cos() as f64;

        // Head & Neck
        self.parts[0].entity.set_pos(Vector3::new(
            pos.x + ss2 * 6.5 * cc_tilt,
            pos.y + head_y_offset + ss_tilt * 6.5,
            pos.z - cc2 * 6.5 * cc_tilt,
        ));
        self.parts[1].entity.set_pos(Vector3::new(
            pos.x + ss2 * 5.5 * cc_tilt,
            pos.y + head_y_offset + ss_tilt * 5.5,
            pos.z - cc2 * 5.5 * cc_tilt,
        ));

        // Tails
        for i in 0..3 {
            let pi = history.get(12 + i * 2);
            let rot = yaw * (std::f32::consts::PI / 180.0)
                + (pi.y_rot - p5.y_rot).rem_euclid(360.0).to_radians();
            let ss = rot.sin() as f64;
            let cc = rot.cos() as f64;
            let dd = (i + 1) as f64 * 2.0;

            self.parts[3 + i as usize].entity.set_pos(Vector3::new(
                pos.x - (ss1 * 1.5 + ss * dd) * cc_tilt,
                pos.y + (pi.y - p5.y) - (dd + 1.5) * ss_tilt + 1.5,
                pos.z + (cc1 * 1.5 + cc * dd) * cc_tilt,
            ));
        }
    }

    pub fn ai_step(&self) {
        if self.mob_entity.living_entity.entity.is_removed() {
            return;
        }
        self.mob_entity.living_entity.entity.update_last_pos();
        if self.mob_entity.is_no_ai() {
            self.tick_parts();
            return;
        }
        self.ensure_nodes_initialized();
        self.update_flap_time();

        {
            let y = self.mob_entity.living_entity.entity.pos.load().y;
            let yaw = self.mob_entity.living_entity.entity.yaw.load();
            self.flight_history
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record(y, yaw);
        };

        self.tick_growl();
        self.tick_crystal_healing();

        {
            let world = self.mob_entity.living_entity.entity.world.load();
            if let Some(ref fight_mutex) = world.dragon_fight {
                let living = &self.mob_entity.living_entity;
                if let Ok(mut fight) = fight_mutex.lock() {
                    let custom_name = living.entity.custom_name.load();
                    fight.update_dragon(
                        &world,
                        living.entity.entity_uuid,
                        living.health.load(),
                        living.get_max_health(),
                        custom_name.as_ref().as_ref(),
                    );
                }
            }
        }

        let mut phase_type = self.current_phase();
        let mut phase = self.phase_manager.get_phase(phase_type);

        if !phase_type.is_sitting() {
            self.handle_player_collisions();
            self.tick_block_breaking();
        }

        phase.tick(self);
        let updated = self.current_phase();
        if updated != phase_type {
            phase_type = updated;
            phase = self.phase_manager.get_phase(phase_type);
            phase.tick(self);
        }

        if self.mob_entity.living_entity.entity.is_removed() {
            return;
        }

        if !matches!(phase_type, EnderDragonPhase::Dying) && !self.current_phase().is_sitting() {
            let target_location = *self
                .target_location
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(target) = target_location {
                let entity = &self.mob_entity.living_entity.entity;
                let velocity = entity.velocity.load();
                let turn_speed = phase.get_turn_speed(velocity.x.hypot(velocity.z) as f32);
                self.steer_toward(entity.pos.load(), target, phase.get_fly_speed(), turn_speed);
            }
        }

        let entity = &self.mob_entity.living_entity.entity;
        entity.body_yaw.store(entity.yaw.load());
        self.tick_parts();
    }

    pub fn hurt(&self, damage: f32) {
        if self.current_phase().is_sitting() {
            let mut accumulated = self
                .sitting_damage_received
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *accumulated += damage;
            if *accumulated > self.mob_entity.living_entity.get_max_health() * 0.25 {
                *accumulated = 0.0;
                drop(accumulated);
                self.set_phase(EnderDragonPhase::TakingOff);
            }
        }
    }
}

impl Mob for EnderDragonEntity {
    fn mob_can_ride(&self, _vehicle: &dyn EntityBase) -> bool {
        false
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ai_step();
    }

    fn travel(&self, _caller: &dyn EntityBase) -> bool {
        // Dragon flight applies its movement before drag; generic mob travel would move it twice.
        self.current_phase() != EnderDragonPhase::Dying
    }

    fn has_global_tick_reach(&self) -> bool {
        // Phase transitions may read the podium and path nodes across the whole arena.
        true
    }

    fn mob_init_data_tracker(&self) {
        self.sync_phase(self.current_phase());
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("DragonPhase", self.current_phase().network_id());
        nbt.put_float(
            "sitting_damage_received",
            *self
                .sitting_damage_received
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(phase) = nbt.get_int("DragonPhase") {
            self.set_phase(EnderDragonPhase::from_network_id(phase));
        }
        *self
            .sitting_damage_received
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            nbt.get_float("sitting_damage_received").unwrap_or(0.0);
    }

    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        let living = &self.mob_entity.living_entity;
        if living.health.load() <= 0.0 {
            self.set_phase(EnderDragonPhase::Dying);
        }
    }

    fn get_mob_gravity(&self) -> f64 {
        0.0
    }

    fn mob_knockback(
        &self,
        _power: f64,
        _x: f64,
        _z: f64,
        _source: Option<&Entity>,
        _damage: f32,
        _comes_from_effect: bool,
    ) -> bool {
        self.phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_sitting()
    }
}

pub trait Vector3Ext {
    fn distance_squared(self, other: Self) -> f64;
}

impl Vector3Ext for Vector3<f64> {
    fn distance_squared(self, other: Self) -> f64 {
        let (dx, dy, dz) = (self.x - other.x, self.y - other.y, self.z - other.z);
        dx * dx + dy * dy + dz * dz
    }
}
