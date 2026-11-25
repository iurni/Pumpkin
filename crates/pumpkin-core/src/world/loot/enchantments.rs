use std::borrow::Cow;
use std::collections::HashSet;

use pumpkin_data::Enchantment;
use pumpkin_data::data_component_impl::{EnchantableImpl, StoredEnchantmentsImpl};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{RegistryKey, get_tag_values};
use pumpkin_util::random::{RandomImpl, xoroshiro128::Xoroshiro};

use crate::{data::datapack::DatapackManager, world::World};

pub(super) fn enchant_with_levels(
    stack: &mut ItemStack,
    level: i32,
    options: Option<&[Cow<'static, str>]>,
    world: Option<&World>,
    rng: &mut Xoroshiro,
) {
    let is_book = stack.item == &Item::BOOK;
    let selected = select_enchantments(stack, level, options, world, rng);
    if is_book {
        *stack = ItemStack::new(1, &Item::ENCHANTED_BOOK);
        if !selected.is_empty() {
            stack.set_data_component(StoredEnchantmentsImpl {
                enchantment: Cow::Owned(selected),
            });
        }
    } else {
        for (enchantment, level) in selected {
            stack.enchant(enchantment, level);
        }
    }
}

fn select_enchantments(
    stack: &ItemStack,
    level: i32,
    options: Option<&[Cow<'static, str>]>,
    world: Option<&World>,
    rng: &mut Xoroshiro,
) -> Vec<(&'static Enchantment, i32)> {
    let Some(enchantable) = stack.get_data_component::<EnchantableImpl>() else {
        return Vec::new();
    };
    let bound = enchantable.value.max(0) / 4 + 1;
    let level = level.saturating_add(1 + rng.next_bounded_i32(bound) + rng.next_bounded_i32(bound));
    let bonus = (rng.next_f32() + rng.next_f32() - 1.0) * 0.15;
    let mut cost = (level as f32 + level as f32 * bonus).round().max(1.0) as i32;
    let mut available: Vec<_> = enchantment_options(options, world)
        .into_iter()
        .filter(|enchantment| stack.item == &Item::BOOK || enchantment.is_primary_item(stack.item))
        .filter_map(|enchantment| {
            (1..=enchantment.max_level)
                .rev()
                .find(|level| {
                    (enchantment.min_cost.calculate(*level)
                        ..=enchantment.max_cost.calculate(*level))
                        .contains(&cost)
                })
                .map(|level| (enchantment, level))
        })
        .collect();
    let mut selected = Vec::new();
    while !available.is_empty() {
        let weight = available
            .iter()
            .map(|(enchantment, _)| enchantment.weight)
            .sum();
        let mut choice = rng.next_bounded_i32(weight);
        let chosen = available
            .iter()
            .copied()
            .find(|(enchantment, _)| {
                choice -= enchantment.weight;
                choice < 0
            })
            .expect("positive enchantment weights cover the draw");
        selected.push(chosen);
        if rng.next_bounded_i32(50) > cost {
            break;
        }
        available.retain(|(enchantment, _)| enchantment.are_compatible(chosen.0));
        cost /= 2;
    }
    selected
}

fn enchantment_options(
    options: Option<&[Cow<'static, str>]>,
    world: Option<&World>,
) -> Vec<&'static Enchantment> {
    let Some(options) = options else {
        return Enchantment::all().copied().collect();
    };
    let server = world.and_then(|world| world.server.upgrade());
    let datapacks = server
        .as_ref()
        .map(|server| server.datapack_manager.as_ref());
    let mut enchantments = Vec::new();
    let mut seen_tags = HashSet::new();
    for option in options {
        resolve_option(option, datapacks, &mut seen_tags, &mut enchantments);
    }
    let mut seen_ids = HashSet::new();
    enchantments.retain(|enchantment| seen_ids.insert(enchantment.id));
    enchantments
}

fn resolve_option(
    option: &str,
    datapacks: Option<&DatapackManager>,
    seen_tags: &mut HashSet<String>,
    enchantments: &mut Vec<&'static Enchantment>,
) {
    let Some(tag) = option.strip_prefix('#') else {
        enchantments.extend(Enchantment::from_name(option));
        return;
    };
    let tag = if tag.contains(':') {
        tag.to_owned()
    } else {
        format!("minecraft:{tag}")
    };
    if !seen_tags.insert(tag.clone()) {
        return;
    }
    if let Some(entries) = datapacks.and_then(|packs| packs.get_tag_entries("enchantment", &tag)) {
        for entry in entries {
            resolve_option(&entry, datapacks, seen_tags, enchantments);
        }
    } else if let Some(entries) = get_tag_values(RegistryKey::Enchantment, &tag) {
        enchantments.extend(
            entries
                .iter()
                .filter_map(|entry| Enchantment::from_name(entry)),
        );
    }
}
