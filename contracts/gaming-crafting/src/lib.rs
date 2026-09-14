// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

//! # Gaming Item Crafting & Durability Degradation Engine
//!
//! An on-chain Soroban smart contract that models:
//! - **Item definitions** – base stats, category, and max durability.
//! - **Recipes** – ordered ingredient lists that can be crafted into a new item.
//! - **Player inventories** – items owned by each player address.
//! - **Crafting** – burns ingredients and mints the crafted item with
//!   pseudo-randomly rolled attributes (attack, defence, speed, magic).
//! - **Durability degradation** – items lose durability each time they are used;
//!   when durability reaches zero the item is destroyed.
//!
//! ## Pseudo-random attribute generation
//! Soroban contracts cannot access external randomness directly.  We derive a
//! deterministic seed from the XOR of:
//!   - the current ledger sequence number (`env.ledger().sequence()`),
//!   - the crafter's address bytes (first 8 bytes, zero-padded),
//!   - the item definition ID,
//!   - the recipe ID.
//!
//! Each rolled attribute stays within the range `[base_stat, base_stat + range]`
//! defined on the `ItemDef`.  This is **deterministic and verifiable on-chain**
//! while being practically unpredictable to the user at transaction submission
//! time (since the ledger sequence is not yet known).
//!
//! ## Storage layout
//! | Key pattern                        | Value           |
//! |------------------------------------|-----------------|
//! | `(ADMIN,)`                         | `Address`       |
//! | `(PAUSED,)`                        | `bool`          |
//! | `(ITEM_DEF, item_def_id)`          | `ItemDef`       |
//! | `(ITEM_DEF_COUNT,)`                | `u32`           |
//! | `(RECIPE, recipe_id)`              | `Recipe`        |
//! | `(RECIPE_COUNT,)`                  | `u32`           |
//! | `(INV, player, slot_index)`        | `ItemInstance`  |
//! | `(INV_SIZE, player)`               | `u32`           |
//!
//! ## Events
//! `item_def_added`, `recipe_added`, `crafted`, `item_used`, `item_destroyed`

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, vec, Address, Env, Symbol,
    Vec,
};

// ── Error codes ───────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    /// Contract was already initialised.
    AlreadyInitialized = 1,
    /// Contract is paused.
    Paused = 2,
    /// Caller is not the admin.
    Unauthorized = 3,
    /// Requested item definition does not exist.
    ItemDefNotFound = 4,
    /// Requested recipe does not exist.
    RecipeNotFound = 5,
    /// Player does not own a required ingredient.
    MissingIngredient = 6,
    /// Crafting would exceed the player's inventory capacity.
    InventoryFull = 7,
    /// Requested inventory slot is empty.
    SlotEmpty = 8,
    /// Item's durability is already zero.
    ItemBroken = 9,
    /// Contract was not yet initialised.
    NotInitialized = 10,
    /// A recipe ingredient list is empty.
    EmptyRecipe = 11,
    /// Item definition name is empty.
    EmptyName = 12,
}

// ── Domain types ──────────────────────────────────────────────────────────────

/// Category tag for an item.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum ItemCategory {
    Weapon,
    Armour,
    Consumable,
    Accessory,
    Material,
}

/// Immutable blueprint for a class of items.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ItemDef {
    /// Auto-assigned identifier.
    pub id: u32,
    /// Display name (max ~64 chars recommended).
    pub name: Symbol,
    pub category: ItemCategory,
    /// Maximum durability when newly crafted / found.
    pub max_durability: u32,
    /// Base attack stat (before random roll).
    pub base_attack: u32,
    /// Base defence stat.
    pub base_defence: u32,
    /// Base speed stat.
    pub base_speed: u32,
    /// Base magic stat.
    pub base_magic: u32,
    /// Each rolled stat is drawn from `[base, base + stat_range]`.
    pub stat_range: u32,
}

/// A crafting recipe: burns the listed item definition IDs and produces
/// `output_item_def_id`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Recipe {
    /// Auto-assigned identifier.
    pub id: u32,
    /// Item definition IDs of required ingredients.
    pub ingredients: Vec<u32>,
    /// Definition ID of the item produced.
    pub output_item_def_id: u32,
}

/// A concrete item instance owned by a player.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ItemInstance {
    /// References the `ItemDef` that defines this item's type.
    pub item_def_id: u32,
    /// Remaining uses before the item is destroyed.
    pub durability: u32,
    /// Rolled attack value (may exceed base_attack).
    pub attack: u32,
    /// Rolled defence value.
    pub defence: u32,
    /// Rolled speed value.
    pub speed: u32,
    /// Rolled magic value.
    pub magic: u32,
}

// ── Storage keys ──────────────────────────────────────────────────────────────

const ADMIN: Symbol = symbol_short!("ADMIN");
const PAUSED: Symbol = symbol_short!("PAUSED");
const ITEM_DEF: Symbol = symbol_short!("ITEM_DEF");
const ITEM_DEF_CNT: Symbol = symbol_short!("ID_CNT");
const RECIPE: Symbol = symbol_short!("RECIPE");
const RECIPE_CNT: Symbol = symbol_short!("RC_CNT");
const INV: Symbol = symbol_short!("INV");
const INV_SIZE: Symbol = symbol_short!("INV_SZ");
const MAX_INVENTORY: u32 = 100;

// ── Storage helpers ───────────────────────────────────────────────────────────

fn get_admin(env: &Env) -> Address {
    env.storage().instance().get(&ADMIN).unwrap()
}

fn is_initialized(env: &Env) -> bool {
    env.storage().instance().has(&ADMIN)
}

fn is_paused(env: &Env) -> bool {
    env.storage().instance().get(&PAUSED).unwrap_or(false)
}

fn require_not_paused(env: &Env) -> Result<(), Error> {
    if is_paused(env) {
        Err(Error::Paused)
    } else {
        Ok(())
    }
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), Error> {
    if get_admin(env) != *caller {
        Err(Error::Unauthorized)
    } else {
        Ok(())
    }
}

fn get_item_def_count(env: &Env) -> u32 {
    env.storage().instance().get(&ITEM_DEF_CNT).unwrap_or(0)
}

fn get_recipe_count(env: &Env) -> u32 {
    env.storage().instance().get(&RECIPE_CNT).unwrap_or(0)
}

fn get_item_def(env: &Env, id: u32) -> Result<ItemDef, Error> {
    env.storage()
        .persistent()
        .get(&(ITEM_DEF, id))
        .ok_or(Error::ItemDefNotFound)
}

fn get_recipe(env: &Env, id: u32) -> Result<Recipe, Error> {
    env.storage()
        .persistent()
        .get(&(RECIPE, id))
        .ok_or(Error::RecipeNotFound)
}

fn get_inv_size(env: &Env, player: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&(INV_SIZE, player.clone()))
        .unwrap_or(0)
}

fn get_inv_slot(env: &Env, player: &Address, slot: u32) -> Option<ItemInstance> {
    env.storage().persistent().get(&(INV, player.clone(), slot))
}

fn set_inv_slot(env: &Env, player: &Address, slot: u32, item: &ItemInstance) {
    env.storage()
        .persistent()
        .set(&(INV, player.clone(), slot), item);
}

fn remove_inv_slot(env: &Env, player: &Address, slot: u32) {
    env.storage()
        .persistent()
        .remove(&(INV, player.clone(), slot));
}

// ── Pseudo-random helper ──────────────────────────────────────────────────────

/// Derive a deterministic u64 seed from ledger sequence + address bytes +
/// item_def_id + recipe_id.  The seed advances with a simple LCG for each
/// stat rolled.
fn derive_seed(env: &Env, crafter: &Address, item_def_id: u32, recipe_id: u32) -> u64 {
    let seq = env.ledger().sequence() as u64;
    let ts = env.ledger().timestamp();
    let addr_val = crafter.to_val().get_payload();

    seq ^ ts ^ addr_val ^ ((item_def_id as u64).wrapping_shl(32)) ^ (recipe_id as u64)
}

/// Advance the LCG seed and return a value in `[0, range]`.
fn roll(seed: &mut u64, range: u32) -> u32 {
    // Knuth multiplicative LCG (64-bit variant).
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    if range == 0 {
        return 0;
    }
    ((*seed >> 33) as u32) % (range + 1)
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct GamingCrafting;

#[contractimpl]
impl GamingCrafting {
    // ── Administration ────────────────────────────────────────────────────────

    /// Initialise the contract. Can only be called once.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if is_initialized(&env) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&ADMIN, &admin);
        env.storage().instance().set(&ITEM_DEF_CNT, &0u32);
        env.storage().instance().set(&RECIPE_CNT, &0u32);
        env.events()
            .publish((symbol_short!("init"),), admin.clone());
        Ok(())
    }

    /// Pause or unpause the contract. Admin only.
    pub fn set_paused(env: Env, admin: Address, paused: bool) -> Result<(), Error> {
        if !is_initialized(&env) {
            return Err(Error::NotInitialized);
        }
        admin.require_auth();
        require_admin(&env, &admin)?;
        env.storage().instance().set(&PAUSED, &paused);
        let evt = if paused {
            symbol_short!("paused")
        } else {
            symbol_short!("unpaused")
        };
        env.events().publish((evt,), admin);
        Ok(())
    }

    // ── Item definitions ──────────────────────────────────────────────────────

    /// Register a new item definition.  Returns the assigned `item_def_id`.
    pub fn add_item_def(
        env: Env,
        admin: Address,
        name: Symbol,
        category: ItemCategory,
        max_durability: u32,
        base_attack: u32,
        base_defence: u32,
        base_speed: u32,
        base_magic: u32,
        stat_range: u32,
    ) -> Result<u32, Error> {
        if !is_initialized(&env) {
            return Err(Error::NotInitialized);
        }
        require_not_paused(&env)?;
        admin.require_auth();
        require_admin(&env, &admin)?;

        let id = get_item_def_count(&env);
        let def = ItemDef {
            id,
            name: name.clone(),
            category,
            max_durability,
            base_attack,
            base_defence,
            base_speed,
            base_magic,
            stat_range,
        };
        env.storage().persistent().set(&(ITEM_DEF, id), &def);
        env.storage().instance().set(&ITEM_DEF_CNT, &(id + 1));

        env.events()
            .publish((symbol_short!("item_def"),), (id, name));
        Ok(id)
    }

    /// Retrieve an item definition by ID.
    pub fn get_item_def(env: Env, item_def_id: u32) -> Result<ItemDef, Error> {
        get_item_def(&env, item_def_id)
    }

    /// Total number of registered item definitions.
    pub fn item_def_count(env: Env) -> u32 {
        get_item_def_count(&env)
    }

    // ── Recipes ───────────────────────────────────────────────────────────────

    /// Register a crafting recipe.  Returns the assigned `recipe_id`.
    ///
    /// * `ingredients` – list of item_def_ids (at least one required).
    /// * `output_item_def_id` – item_def_id produced by the recipe.
    pub fn add_recipe(
        env: Env,
        admin: Address,
        ingredients: Vec<u32>,
        output_item_def_id: u32,
    ) -> Result<u32, Error> {
        if !is_initialized(&env) {
            return Err(Error::NotInitialized);
        }
        require_not_paused(&env)?;
        admin.require_auth();
        require_admin(&env, &admin)?;

        if ingredients.is_empty() {
            return Err(Error::EmptyRecipe);
        }
        // Validate output item exists.
        get_item_def(&env, output_item_def_id)?;
        // Validate each ingredient item exists.
        for i in 0..ingredients.len() {
            get_item_def(&env, ingredients.get_unchecked(i))?;
        }

        let id = get_recipe_count(&env);
        let recipe = Recipe {
            id,
            ingredients,
            output_item_def_id,
        };
        env.storage().persistent().set(&(RECIPE, id), &recipe);
        env.storage().instance().set(&RECIPE_CNT, &(id + 1));

        env.events()
            .publish((symbol_short!("recipe"),), (id, output_item_def_id));
        Ok(id)
    }

    /// Retrieve a recipe by ID.
    pub fn get_recipe(env: Env, recipe_id: u32) -> Result<Recipe, Error> {
        get_recipe(&env, recipe_id)
    }

    /// Total number of registered recipes.
    pub fn recipe_count(env: Env) -> u32 {
        get_recipe_count(&env)
    }

    // ── Crafting ──────────────────────────────────────────────────────────────

    /// Craft an item using `recipe_id`.
    ///
    /// The caller must own at least one item of each ingredient type.  The
    /// **first matching slot** is consumed for each ingredient.  The crafted
    /// item is appended to the caller's inventory.
    ///
    /// Returns the inventory slot index of the newly crafted item.
    pub fn craft(env: Env, player: Address, recipe_id: u32) -> Result<u32, Error> {
        if !is_initialized(&env) {
            return Err(Error::NotInitialized);
        }
        require_not_paused(&env)?;
        player.require_auth();

        let recipe = get_recipe(&env, recipe_id)?;
        let output_def = get_item_def(&env, recipe.output_item_def_id)?;

        let inv_size = get_inv_size(&env, &player);
        if inv_size >= MAX_INVENTORY {
            return Err(Error::InventoryFull);
        }

        // ── Consume ingredients ───────────────────────────────────────────────
        // For each ingredient def_id, find and remove the first matching slot.
        for i in 0..recipe.ingredients.len() {
            let needed_def = recipe.ingredients.get_unchecked(i);
            let mut found = false;
            for slot in 0..inv_size {
                if let Some(item) = get_inv_slot(&env, &player, slot) {
                    if item.item_def_id == needed_def {
                        remove_inv_slot(&env, &player, slot);
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return Err(Error::MissingIngredient);
            }
        }

        // ── Compact inventory after removing ingredients ───────────────────────
        // Re-index remaining items into contiguous slots [0..new_size).
        let mut new_size: u32 = 0;
        let mut compact: Vec<ItemInstance> = vec![&env];
        for slot in 0..inv_size {
            if let Some(item) = get_inv_slot(&env, &player, slot) {
                compact.push_back(item);
            }
        }
        // Clear old slots.
        for slot in 0..inv_size {
            remove_inv_slot(&env, &player, slot);
        }
        // Write compacted items.
        for idx in 0..compact.len() {
            set_inv_slot(&env, &player, idx, &compact.get_unchecked(idx));
            new_size = idx + 1;
        }

        // ── Roll stats ────────────────────────────────────────────────────────
        let mut seed = derive_seed(&env, &player, output_def.id, recipe_id);
        let crafted = ItemInstance {
            item_def_id: output_def.id,
            durability: output_def.max_durability,
            attack: output_def.base_attack + roll(&mut seed, output_def.stat_range),
            defence: output_def.base_defence + roll(&mut seed, output_def.stat_range),
            speed: output_def.base_speed + roll(&mut seed, output_def.stat_range),
            magic: output_def.base_magic + roll(&mut seed, output_def.stat_range),
        };

        // Append crafted item.
        let new_slot = new_size;
        set_inv_slot(&env, &player, new_slot, &crafted);
        env.storage()
            .persistent()
            .set(&(INV_SIZE, player.clone()), &(new_slot + 1));

        env.events().publish(
            (symbol_short!("crafted"),),
            (player, recipe_id, output_def.id, new_slot),
        );
        Ok(new_slot)
    }

    // ── Item usage & durability ───────────────────────────────────────────────

    /// Use the item at `slot` in the caller's inventory, reducing its
    /// durability by `uses` (default: 1).  If durability reaches zero the item
    /// is destroyed and the inventory is compacted.
    ///
    /// Returns the remaining durability (0 means the item was just destroyed).
    pub fn use_item(env: Env, player: Address, slot: u32, uses: u32) -> Result<u32, Error> {
        if !is_initialized(&env) {
            return Err(Error::NotInitialized);
        }
        require_not_paused(&env)?;
        player.require_auth();

        let mut item = get_inv_slot(&env, &player, slot).ok_or(Error::SlotEmpty)?;
        if item.durability == 0 {
            return Err(Error::ItemBroken);
        }

        let actual_uses = uses.max(1);
        let remaining = item.durability.saturating_sub(actual_uses);

        if remaining == 0 {
            // Destroy item and compact inventory.
            let inv_size = get_inv_size(&env, &player);
            remove_inv_slot(&env, &player, slot);

            let mut compacted: Vec<ItemInstance> = vec![&env];
            for s in 0..inv_size {
                if s == slot {
                    continue;
                }
                if let Some(i) = get_inv_slot(&env, &player, s) {
                    compacted.push_back(i);
                }
            }
            for s in 0..inv_size {
                remove_inv_slot(&env, &player, s);
            }
            let new_size = compacted.len();
            for idx in 0..new_size {
                set_inv_slot(&env, &player, idx, &compacted.get_unchecked(idx));
            }
            env.storage()
                .persistent()
                .set(&(INV_SIZE, player.clone()), &new_size);

            env.events().publish(
                (symbol_short!("destroyed"),),
                (player, item.item_def_id, slot),
            );
            return Ok(0);
        }

        item.durability = remaining;
        set_inv_slot(&env, &player, slot, &item);

        env.events()
            .publish((symbol_short!("used"),), (player, slot, remaining));
        Ok(remaining)
    }

    // ── Inventory queries ─────────────────────────────────────────────────────

    /// Return the number of items in the player's inventory.
    pub fn inventory_size(env: Env, player: Address) -> u32 {
        get_inv_size(&env, &player)
    }

    /// Return the item at a specific inventory slot.
    pub fn get_item(env: Env, player: Address, slot: u32) -> Result<ItemInstance, Error> {
        get_inv_slot(&env, &player, slot).ok_or(Error::SlotEmpty)
    }

    // ── Admin: grant item ─────────────────────────────────────────────────────

    /// Admin may grant an item directly to a player (e.g. for onboarding or
    /// rewards).  Stats are also pseudo-randomly rolled.
    pub fn grant_item(
        env: Env,
        admin: Address,
        player: Address,
        item_def_id: u32,
    ) -> Result<u32, Error> {
        if !is_initialized(&env) {
            return Err(Error::NotInitialized);
        }
        require_not_paused(&env)?;
        admin.require_auth();
        require_admin(&env, &admin)?;

        let def = get_item_def(&env, item_def_id)?;
        let inv_size = get_inv_size(&env, &player);
        if inv_size >= MAX_INVENTORY {
            return Err(Error::InventoryFull);
        }

        let mut seed = derive_seed(&env, &player, def.id, u32::MAX);
        let item = ItemInstance {
            item_def_id: def.id,
            durability: def.max_durability,
            attack: def.base_attack + roll(&mut seed, def.stat_range),
            defence: def.base_defence + roll(&mut seed, def.stat_range),
            speed: def.base_speed + roll(&mut seed, def.stat_range),
            magic: def.base_magic + roll(&mut seed, def.stat_range),
        };

        set_inv_slot(&env, &player, inv_size, &item);
        env.storage()
            .persistent()
            .set(&(INV_SIZE, player.clone()), &(inv_size + 1));

        env.events()
            .publish((symbol_short!("granted"),), (player, item_def_id, inv_size));
        Ok(inv_size)
    }
}
