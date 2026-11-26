use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU8, Ordering};

use crate::entity::{Entity, EntityBase, living::LivingEntity, player::Player};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{
    Block,
    attributes::Attributes,
    damage::DamageType,
    data_component_impl::{EquipmentSlot, EquipmentType, EquippableImpl},
    entity::{EntityStatus, EntityType},
    item::Item,
    sound::{Sound, SoundCategory},
    tag::{self, Taggable},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use pumpkin_util::Hand;
use pumpkin_util::math::{
    boundingbox::EntityDimensions, euler_angle::EulerAngle, vector3::Vector3,
};

#[derive(Debug, Clone, Copy)]
pub struct PackedRotation {
    pub head: EulerAngle,
    pub body: EulerAngle,
    pub left_arm: EulerAngle,
    pub right_arm: EulerAngle,
    pub left_leg: EulerAngle,
    pub right_leg: EulerAngle,
}

impl Default for PackedRotation {
    fn default() -> Self {
        Self {
            head: EulerAngle::new(0.0, 0.0, 0.0),
            body: EulerAngle::new(0.0, 0.0, 0.0),
            left_arm: EulerAngle::new(-10.0, 0.0, -10.0),
            right_arm: EulerAngle::new(-15.0, 0.0, 10.0),
            left_leg: EulerAngle::new(-1.0, 0.0, -1.0),
            right_leg: EulerAngle::new(1.0, 0.0, 1.0),
        }
    }
}

impl From<PackedRotation> for NbtTag {
    fn from(val: PackedRotation) -> Self {
        let mut compound = NbtCompound::new();
        compound.put("Head", val.head);
        compound.put("Body", val.body);
        compound.put("LeftArm", val.left_arm);
        compound.put("RightArm", val.right_arm);
        compound.put("LeftLeg", val.left_leg);
        compound.put("RightLeg", val.right_leg);
        Self::Compound(compound)
    }
}

impl From<NbtTag> for PackedRotation {
    #[expect(clippy::unnecessary_fallible_conversions)]
    fn from(tag: NbtTag) -> Self {
        if let NbtTag::Compound(compound) = tag {
            fn get_rotation(
                compound: &NbtCompound,
                key: &'static str,
                default: EulerAngle,
            ) -> EulerAngle {
                compound
                    .get(key)
                    .and_then(|tag| tag.clone().try_into().ok())
                    .unwrap_or(default)
            }

            let default = Self::default();

            Self {
                head: get_rotation(&compound, "Head", default.head),
                body: get_rotation(&compound, "Body", default.body),
                left_arm: get_rotation(&compound, "LeftArm", default.left_arm),
                right_arm: get_rotation(&compound, "RightArm", default.right_arm),
                left_leg: get_rotation(&compound, "LeftLeg", default.left_leg),
                right_leg: get_rotation(&compound, "RightLeg", default.right_leg),
            }
        } else {
            Self::default()
        }
    }
}

pub struct ArmorStandEntity {
    living_entity: LivingEntity,

    armor_stand_flags: AtomicU8,
    last_hit_time: AtomicI64,
    disabled_slots: AtomicI32,

    rotation: AtomicCell<PackedRotation>,
}

impl ArmorStandEntity {
    pub fn new(entity: Entity) -> Self {
        let living_entity = LivingEntity::new(entity);
        let packed_rotation = PackedRotation::default();

        Self {
            living_entity,
            armor_stand_flags: AtomicU8::new(0),
            last_hit_time: AtomicI64::new(0),
            disabled_slots: AtomicI32::new(0),
            rotation: AtomicCell::new(packed_rotation),
        }
    }

    pub fn set_small(&self, small: bool) {
        self.set_bit_field(ArmorStandFlags::Small, small);
    }

    pub fn is_small(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::Small as u8) != 0
    }

    pub fn set_show_arms(&self, show_arms: bool) {
        self.set_bit_field(ArmorStandFlags::ShowArms, show_arms);
    }

    pub fn should_show_arms(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::ShowArms as u8) != 0
    }

    pub fn set_hide_base_plate(&self, hide_base_plate: bool) {
        self.set_bit_field(ArmorStandFlags::HideBasePlate, hide_base_plate);
    }

    pub fn should_show_base_plate(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::HideBasePlate as u8) == 0
    }

    pub fn set_marker(&self, marker: bool) {
        self.set_bit_field(ArmorStandFlags::Marker, marker);
    }

    pub fn is_marker(&self) -> bool {
        (self.armor_stand_flags.load(Ordering::Relaxed) & ArmorStandFlags::Marker as u8) != 0
    }

    fn set_bit_field(&self, bit_field: ArmorStandFlags, set: bool) {
        let current = self.armor_stand_flags.load(Ordering::Relaxed);
        let new_value = if set {
            current | bit_field as u8
        } else {
            current & !(bit_field as u8)
        };
        self.armor_stand_flags.store(new_value, Ordering::Relaxed);
        self.update_dimensions();
    }

    /// Vanilla `ArmorStand.getDefaultDimensions`: a marker has no size, a small stand half.
    fn update_dimensions(&self) {
        let dimensions = if self.is_marker() {
            EntityDimensions::new(0.0, 0.0, 0.0)
        } else if self.is_small() {
            let size = Entity::type_dimensions(&EntityType::ARMOR_STAND).scale(0.5);
            EntityDimensions::new(size.width, size.height, 0.9875)
        } else {
            Entity::type_dimensions(&EntityType::ARMOR_STAND)
        };
        self.living_entity.entity.set_default_dimensions(dimensions);
    }

    pub fn can_use_slot(&self, slot: &EquipmentSlot) -> bool {
        !matches!(slot, EquipmentSlot::Body(_) | EquipmentSlot::Saddle(_))
            && !self.is_slot_disabled(slot)
    }

    pub fn is_slot_disabled(&self, slot: &EquipmentSlot) -> bool {
        let disabled_slots = self.disabled_slots.load(Ordering::Relaxed);
        let slot_bit = 1 << slot.get_slot_index();

        (disabled_slots & slot_bit) != 0
            || (slot.slot_type() == EquipmentType::Hand && !self.should_show_arms())
    }

    pub fn set_slot_disabled(&self, slot: &EquipmentSlot, disabled: bool) {
        let slot_bit = 1 << slot.get_slot_index();
        let current = self.disabled_slots.load(Ordering::Relaxed);

        let new_val = if disabled {
            current | slot_bit
        } else {
            current & !slot_bit
        };

        self.disabled_slots.store(new_val, Ordering::Relaxed);
    }

    pub fn is_invisible(&self) -> bool {
        self.get_entity().invisible.load(Ordering::Relaxed)
    }

    pub fn pack_rotation(&self) -> PackedRotation {
        self.rotation.load()
    }

    pub fn unpack_rotation(&self, packed: &PackedRotation) {
        self.rotation.store(packed.to_owned());
    }

    /// Selects an occupied slot under the hit point, following vanilla's order.
    fn clicked_slot(&self, hit_y: f64) -> EquipmentSlot {
        let small = self.is_small();
        let scale = self.living_entity.get_attribute_value(&Attributes::SCALE)
            * if small { 0.5 } else { 1.0 };
        let y = hit_y / scale;
        let equipment = self
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let has = |slot: &EquipmentSlot| !equipment.get(slot).is_empty();
        if y >= 0.1 && y < 0.1 + if small { 0.8 } else { 0.45 } && has(&EquipmentSlot::FEET) {
            EquipmentSlot::FEET
        } else if y >= 0.9 + if small { 0.3 } else { 0.0 }
            && y < 0.9 + if small { 1.0 } else { 0.7 }
            && has(&EquipmentSlot::CHEST)
        {
            EquipmentSlot::CHEST
        } else if y >= 0.4 && y < 0.4 + if small { 1.0 } else { 0.8 } && has(&EquipmentSlot::LEGS) {
            EquipmentSlot::LEGS
        } else if y >= 1.6 && has(&EquipmentSlot::HEAD) {
            EquipmentSlot::HEAD
        } else if !has(&EquipmentSlot::MAIN_HAND) && has(&EquipmentSlot::OFF_HAND) {
            EquipmentSlot::OFF_HAND
        } else {
            EquipmentSlot::MAIN_HAND
        }
    }

    fn interact_at(&self, player: &Arc<Player>, held: &mut ItemStack, hit: Vector3<f64>) -> bool {
        if self.is_marker() || held.item == &Item::NAME_TAG || !hit.y.is_finite() {
            return false;
        }
        let item_slot = held
            .get_data_component::<EquippableImpl>()
            .map(|equippable| equippable.slot)
            .filter(|slot| self.can_use_slot(slot))
            .unwrap_or(&EquipmentSlot::MAIN_HAND);
        let slot = if held.is_empty() {
            let clicked = self.clicked_slot(hit.y);
            if self.is_slot_disabled(&clicked) {
                item_slot.clone()
            } else {
                clicked
            }
        } else {
            if self.is_slot_disabled(item_slot) {
                return false;
            }
            item_slot.clone()
        };

        // Check before firing the event, then recheck under the lock afterward.
        {
            let equipment = self
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let old = equipment.get(&slot);
            let disabled = self.disabled_slots.load(Ordering::Relaxed);
            if self.get_entity().is_removed()
                || (held.is_empty() && old.is_empty())
                || (!old.is_empty() && disabled & (1 << (slot.get_slot_index() + 8)) != 0)
                || (old.is_empty() && disabled & (1 << (slot.get_slot_index() + 16)) != 0)
                || (held.item_count > 1 && !old.is_empty())
            {
                return false;
            }
        }

        // Do not invoke plugin callbacks while holding the stand's inventory lock.
        if let Some(server) = self.get_entity().world.load().server.upgrade() {
            let mut event = crate::plugin::api::events::player::player_armor_stand_manipulate::PlayerArmorStandManipulateEvent {
                player: player.clone(),
                armor_stand_id: self.get_entity().entity_id,
                slot: slot.get_slot_index() as u8,
                cancelled: false,
            };
            server.plugin_manager.fire_blocking(&server, &mut event);
            if event.cancelled {
                return true;
            }
        }

        let creative = player.is_creative();
        let changed = {
            let mut equipment = self
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let old = equipment.get(&slot);
            let disabled = self.disabled_slots.load(Ordering::Relaxed);
            if self.get_entity().is_removed()
                || (held.is_empty() && old.is_empty())
                || (!old.is_empty() && disabled & (1 << (slot.get_slot_index() + 8)) != 0)
                || (old.is_empty() && disabled & (1 << (slot.get_slot_index() + 16)) != 0)
            {
                return false;
            }
            let new_stack = if creative && old.is_empty() && !held.is_empty() {
                held.copy_with_count(1)
            } else if !held.is_empty() && held.item_count > 1 {
                if !old.is_empty() {
                    return false;
                }
                held.split(1)
            } else {
                std::mem::replace(held, old)
            };
            equipment.put(&slot, new_stack.clone());
            new_stack
        };
        self.living_entity
            .send_equipment_changes(&[(slot, changed)]);
        true
    }

    fn break_and_drop_items(&self) {
        let entity = self.get_entity();
        //let name = entity.custom_name.unwrap_or(entity.get_name());

        //TODO: i am stupid! let armor_stand_item = ItemStack::new_with_component(1, &Item::ARMOR_STAND, vec![(DataComponent::CustomName, self.get_custom_name())]);
        let armor_stand_item = ItemStack::new(1, &Item::ARMOR_STAND);
        entity
            .world
            .load()
            .drop_stack(&entity.block_pos.load(), armor_stand_item);

        self.on_break();
    }

    fn on_break(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityArmorStandBreak,
            SoundCategory::Neutral,
            &entity.pos.load(),
        );

        let equipment = {
            let mut guard = self
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut guard.equipment)
        };
        for stack in equipment.into_values() {
            if !crate::entity::living::prevents_equipment_drop(&stack) {
                world.drop_stack(&entity.block_pos.load().up(), stack);
            }
        }
    }

    /// Spawns break particles at the armor stand's position.
    fn spawn_break_particles(entity: &Entity) {
        let world = entity.world.load();
        let pos = entity.pos.load();
        let width = entity.width();
        let height = entity.height();

        // ArmorStand.showBreakingParticles
        world.spawn_block_particle(
            Vector3::new(pos.x, pos.y + f64::from(height) / 1.5, pos.z),
            Vector3::new(width / 4.0, height / 4.0, width / 4.0),
            0.05,
            10,
            Block::OAK_PLANKS.default_state.id.as_u16(),
        );
    }
}

impl EntityBase for ArmorStandEntity {
    fn interact_at_with_hand(
        &self,
        player: &Arc<Player>,
        item_stack: &mut ItemStack,
        _hand: Hand,
        position: Vector3<f64>,
    ) -> Option<bool> {
        Some(self.interact_at(player, item_stack, position))
    }
    fn can_hit(&self) -> bool {
        !self.get_entity().is_removed() && !self.is_marker()
    }

    fn can_be_seen_by_anyone(&self) -> bool {
        !self.is_invisible() && !self.is_marker()
    }

    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let disabled_slots = self.disabled_slots.load(Ordering::Relaxed);
        // ...

        nbt.put_bool("Invisible", self.is_invisible());
        nbt.put_bool("Small", self.is_small());
        nbt.put_bool("ShowArms", self.should_show_arms());
        nbt.put_int("DisabledSlots", disabled_slots);
        nbt.put_bool("NoBasePlate", !self.should_show_base_plate());
        if self.is_marker() {
            nbt.put_bool("Marker", true);
        }

        nbt.put("Pose", self.pack_rotation());
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let mut flags = 0u8;
        // ...

        if let Some(invisible) = nbt.get_bool("Invisible")
            && invisible
        {
            self.get_entity().set_invisible(invisible);
        }

        if let Some(small) = nbt.get_bool("Small")
            && small
        {
            flags |= ArmorStandFlags::Small as u8;
        }

        if let Some(show_arms) = nbt.get_bool("ShowArms")
            && show_arms
        {
            flags |= ArmorStandFlags::ShowArms as u8;
        }

        if let Some(disabled_slots) = nbt.get_int("DisabledSlots") {
            self.disabled_slots.store(disabled_slots, Ordering::Relaxed);
        }

        if let Some(no_base_plate) = nbt.get_bool("NoBasePlate") {
            if !no_base_plate {
                flags |= ArmorStandFlags::HideBasePlate as u8;
            }
        } else {
            flags |= ArmorStandFlags::HideBasePlate as u8;
        }

        if let Some(marker) = nbt.get_bool("Marker")
            && marker
        {
            flags |= ArmorStandFlags::Marker as u8;
        }

        self.armor_stand_flags.store(flags, Ordering::Relaxed);
        self.update_dimensions();

        if let Some(pose_tag) = nbt.get("Pose") {
            let packed: PackedRotation = pose_tag.clone().into();
            self.unpack_rotation(&packed);
        }
    }

    fn get_entity(&self) -> &Entity {
        &self.living_entity.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        Some(&self.living_entity)
    }

    fn kill(&self, _caller: &dyn EntityBase) {
        self.get_entity().remove();
        // TODO: emit GameEvent::ENTITY_DIE
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        source: Option<&dyn EntityBase>,
        cause: Option<&dyn EntityBase>,
    ) -> bool {
        let entity = self.get_entity();
        if entity.is_removed() {
            return false;
        }

        let world = entity.world.load();

        let mob_griefing_gamerule = {
            let game_rules = &world.level_info.load().game_rules;
            game_rules.mob_griefing
        };

        // Vanilla: only mob attackers are stopped by `mobGriefing`.
        if !mob_griefing_gamerule && cause.is_some_and(|cause| cause.get_mob().is_some()) {
            return false;
        }

        if damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY) {
            entity.remove();
            return false;
        }

        if entity.is_invulnerable_to(&damage_type, cause) || self.is_invisible() || self.is_marker()
        {
            return false;
        }

        if damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION) {
            self.on_break();
            entity.remove();
            return false;
        }

        // TODO: IGNITES_ARMOR_STANDS (in_fire, campfire) - set on fire
        // TODO: BURNS_ARMOR_STANDS (on_fire) - reduce health

        let can_break = damage_type.has_tag(&tag::DamageType::MINECRAFT_CAN_BREAK_ARMOR_STAND);
        let always_kills =
            damage_type.has_tag(&tag::DamageType::MINECRAFT_ALWAYS_KILLS_ARMOR_STANDS);

        if !can_break && !always_kills {
            return false;
        }

        let attacker = cause.or(source);
        if let Some(attacker) = attacker
            && let Some(player) = attacker.get_player()
        {
            if !player
                .abilities
                .try_lock()
                .is_ok_and(|a| a.allow_modify_world)
            {
                return false;
            } else if player.is_creative() {
                Self::spawn_break_particles(entity);
                entity.remove();
                return true;
            }
        }

        let time = world
            .level_time
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .query_gametime();

        if time - self.last_hit_time.load(Ordering::Relaxed) > 5 && !always_kills {
            world.send_entity_status(entity, EntityStatus::ArmorstandWobble, ActorEventID::None);
            world.play_sound(
                Sound::EntityArmorStandHit,
                SoundCategory::Neutral,
                &entity.block_pos.load().to_f64(),
            );
            self.last_hit_time.store(time, Ordering::Relaxed);
        } else {
            Self::spawn_break_particles(entity);
            world.play_sound(
                Sound::EntityArmorStandBreak,
                SoundCategory::Neutral,
                &entity.block_pos.load().to_f64(),
            );
            self.break_and_drop_items();
            entity.remove();
        }

        true
    }

    fn get_gravity(&self) -> f64 {
        0.08
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub enum ArmorStandFlags {
    /// Small armor stand Flag
    Small = 1,
    /// Show arms Flag
    ShowArms = 4,
    /// Hide base plate fLag
    HideBasePlate = 8,
    /// Marker Flag
    Marker = 16,
}
