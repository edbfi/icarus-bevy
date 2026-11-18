//! Authored bindings, recipes and catalogs only; no packaged row or value.
use super::*;
use crate::native_inventory::{
    InventoryDefinitionV1, ItemDefinitionV1, SlotRuleV1, StackId, StackV1,
};
use crate::native_player_inventory::{PlayerInventoryRecipeV1, PlayerStarterItemV1};

fn names(rows: &[&str]) -> Vec<String> {
    rows.iter().map(|row| (*row).to_owned()).collect()
}

/// Authored catalogs: rows sit at known positions; `Twice` is listed twice.
struct Catalog {
    items: Vec<String>,
    infos: Vec<String>,
    queries: Vec<String>,
}

impl Catalog {
    fn new() -> Self {
        Self {
            items: names(&[
                "ItemZero",
                "AuthoredWater",
                "AuthoredRation",
                "Twice",
                "Twice",
            ]),
            infos: names(&[
                "InfoZero",
                "InfoOne",
                "AuthoredBag",
                "AuthoredBelt",
                "Twice",
                "Twice",
            ]),
            queries: names(&["QueryZero", "AnyAuthored", "WaterOnly", "Twice", "Twice"]),
        }
    }
    fn view(&self) -> ProjectionCatalog<'_> {
        ProjectionCatalog {
            items_static: &self.items,
            inventory_info: &self.infos,
            tag_queries: &self.queries,
        }
    }
}

fn inventory(key: &str, slots: u16, allowed: &[&str]) -> InventoryDefinitionV1 {
    InventoryDefinitionV1 {
        key: key.into(),
        initial_slots: slots,
        slot_rules: (0..slots)
            .map(|_| SlotRuleV1 {
                allowed_items: names(allowed),
            })
            .collect(),
        force_add_when_zero: false,
        overflow_limit: 0,
    }
}

fn recipe_with(inventories: Vec<InventoryDefinitionV1>) -> CompiledPlayerInventoryRecipe {
    CompiledPlayerInventoryRecipe::compile(PlayerInventoryRecipeV1 {
        format: 1,
        items: vec![
            ItemDefinitionV1 {
                key: "water".into(),
                stack_limit: 10,
            },
            ItemDefinitionV1 {
                key: "ration".into(),
                stack_limit: 1,
            },
        ],
        inventories,
        starter: vec![PlayerStarterItemV1 {
            inventory: "bag".into(),
            requested_slot: 0,
            item: "water".into(),
            count: 3,
        }],
    })
    .unwrap()
}

fn recipe() -> CompiledPlayerInventoryRecipe {
    recipe_with(vec![
        inventory("bag", 3, &["water", "ration"]),
        inventory("belt", 1, &["ration"]),
    ])
}

fn binding() -> PlayerInventoryProjectionBinding {
    PlayerInventoryProjectionBinding {
        format: 1,
        inventories: vec![
            InventoryBinding {
                inventory: "belt".into(),
                info_row: "AuthoredBelt".into(),
                slot_queries: names(&["AnyAuthored"]),
            },
            InventoryBinding {
                inventory: "bag".into(),
                info_row: "AuthoredBag".into(),
                slot_queries: names(&["WaterOnly", "AnyAuthored", "AnyAuthored"]),
            },
        ],
        items: vec![
            ItemBinding {
                item: "water".into(),
                row: "AuthoredWater".into(),
                count_property: true,
                dynamic: vec![
                    DynamicProperty {
                        property: 12,
                        value: 900,
                    },
                    DynamicProperty {
                        property: 0,
                        value: 0,
                    },
                ],
                unit_weight: None,
            },
            ItemBinding {
                item: "ration".into(),
                row: "AuthoredRation".into(),
                count_property: false,
                dynamic: Vec::new(),
                unit_weight: None,
            },
        ],
    }
}

fn compiled() -> PlayerInventoryProjection {
    PlayerInventoryProjection::compile(&binding(), &recipe(), Catalog::new().view(), &[0]).unwrap()
}

fn weighted_binding(water: u32, ration: u32) -> PlayerInventoryProjectionBinding {
    let mut weighted = binding();
    weighted.format = 2;
    weighted.items[0].unit_weight = Some(water);
    weighted.items[1].unit_weight = Some(ration);
    weighted
}

fn refusal(
    binding: &PlayerInventoryProjectionBinding,
    recipe: &CompiledPlayerInventoryRecipe,
    catalog: ProjectionCatalog<'_>,
    opened: &[u8],
) -> String {
    PlayerInventoryProjection::compile(binding, recipe, catalog, opened)
        .unwrap_err()
        .to_string()
}

fn stack(id: u64, item: &str, count: u32) -> Option<StackV1> {
    Some(StackV1 {
        id: StackId::new(id).unwrap(),
        item: item.into(),
        count,
    })
}

fn bag(bag_slots: Vec<Option<StackV1>>, belt: Option<StackV1>) -> Vec<InventoryStateV1> {
    vec![
        InventoryStateV1 {
            key: "bag".into(),
            slots: bag_slots,
            overflow: Vec::new(),
        },
        InventoryStateV1 {
            key: "belt".into(),
            slots: vec![belt],
            overflow: Vec::new(),
        },
    ]
}

#[test]
fn binding_is_strict_bounded_json() {
    let text = serde_json::to_string(&binding()).unwrap();
    assert!(!text.contains("unit_weight"));
    assert_eq!(
        PlayerInventoryProjectionBinding::parse(&text).unwrap(),
        binding()
    );
    for bad in [
        r#"{"format":1,"inventories":[],"items":[],"extra":1}"#,
        r#"{"format":1,"inventories":[{"inventory":"bag","info_row":"A","slot_queries":[],"weight":1}],"items":[]}"#,
        r#"{"format":1,"inventories":[],"items":[{"item":"water","row":"A","count_property":true,"dynamic":[{"property":0,"value":0,"text":"x"}]}]}"#,
        r#"{"format":1,"inventories":[]}"#,
        r#"{"format":1,"format":1,"inventories":[],"items":[]}"#,
    ] {
        assert!(
            PlayerInventoryProjectionBinding::parse(bad).is_err(),
            "{bad}"
        );
    }
    let oversize = format!("{text}{}", " ".repeat(MAX_BINDING_BYTES));
    assert!(
        PlayerInventoryProjectionBinding::parse(&oversize)
            .unwrap_err()
            .to_string()
            .contains("byte limit")
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("binding.json");
    std::fs::write(&path, &text).unwrap();
    assert_eq!(
        PlayerInventoryProjectionBinding::load(&path).unwrap(),
        binding()
    );
    assert!(PlayerInventoryProjectionBinding::load(directory.path()).is_err());
    std::fs::write(&path, &oversize).unwrap();
    assert!(PlayerInventoryProjectionBinding::load(&path).is_err());
}

#[test]
fn weighted_bindings_require_explicit_bounded_integers() {
    let weighted = weighted_binding(0, 5);
    let text = serde_json::to_string(&weighted).unwrap();
    assert_eq!(
        PlayerInventoryProjectionBinding::parse(&text).unwrap(),
        weighted
    );
    for invalid in ["null", "-1", "0.5", "4294967296"] {
        let bad = text.replacen(
            "\"unit_weight\":0",
            &format!("\"unit_weight\":{invalid}"),
            1,
        );
        assert!(
            PlayerInventoryProjectionBinding::parse(&bad).is_err(),
            "{bad}"
        );
    }
    let recipe = recipe();
    let catalog = Catalog::new();
    let check = |binding, message| {
        let error = refusal(&binding, &recipe, catalog.view(), &[0]);
        assert!(error.contains(message), "{error}");
    };
    let mut legacy = weighted.clone();
    legacy.format = 1;
    check(legacy, "forbidden in format 1");
    let mut missing = weighted;
    missing.items[0].unit_weight = None;
    check(missing, "required in format 2");
    check(weighted_binding(0, 1 << 31), "unit_weight exceeds");
    check(
        weighted_binding(214_748_365, 0),
        "maximum CurrentWeight exceeds",
    );
    check(
        weighted_binding(100_000_000, 0),
        "maximum CurrentWeight exceeds",
    );
    // The exact upper limit is valid for a single-stack belt. It would be
    // too large if the bag also permitted this item in several slots.
    let restricted = recipe_with(vec![
        inventory("bag", 3, &["water"]),
        inventory("belt", 1, &["ration"]),
    ]);
    let projection = PlayerInventoryProjection::compile(
        &weighted_binding(0, i32::MAX as u32),
        &restricted,
        catalog.view(),
        &[0],
    )
    .unwrap();
    let mut peer = PeerBagProjection::opened(&projection, 0).unwrap();
    let full_belt = bag(vec![None, None, None], stack(1, "ration", 1));
    peer.observe(1, 1, || projection.project(&full_belt))
        .unwrap();
    assert_eq!(peer.deltas(1).unwrap()[0].weight, Some(i32::MAX));
}

#[test]
fn shipped_example_binding_parses() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("config/player-inventory-projection.example.json");
    let example = PlayerInventoryProjectionBinding::load(&path).unwrap();
    assert_eq!(example.format, 2);
    let recipe = CompiledPlayerInventoryRecipe::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config/player-inventory-consumables.example.json"),
    )
    .unwrap();
    // Its rows are placeholders an operator replaces; an authored catalog
    // naming them binds the shipped recipe completely.
    let rows = |names: Vec<&String>| names.into_iter().cloned().collect::<Vec<_>>();
    let items = rows(example.items.iter().map(|item| &item.row).collect());
    let infos = rows(example.inventories.iter().map(|i| &i.info_row).collect());
    let queries = rows(
        example
            .inventories
            .iter()
            .flat_map(|i| &i.slot_queries)
            .collect(),
    );
    let mut queries = queries;
    queries.dedup();
    PlayerInventoryProjection::compile(
        &example,
        &recipe,
        ProjectionCatalog {
            items_static: &items,
            inventory_info: &infos,
            tag_queries: &queries,
        },
        &[],
    )
    .unwrap();
}

#[test]
fn rows_resolve_to_catalog_positions_and_open_empty_at_the_captured_key() {
    let projection = compiled();
    // Recipe-key order: bag, then belt.
    let opened = projection.opened_inventories();
    assert_eq!(opened.len(), 2);
    assert_eq!((opened[0].info_row, opened[1].info_row), (2, 3));
    assert_eq!(opened[0].current_weight, None, "weight is never inferred");
    assert_eq!(opened[0].slots.replication_key, 2);
    assert_eq!(
        opened[0].slots.items,
        vec![
            InventorySlot::empty(2, 0),
            InventorySlot::empty(1, 1),
            InventorySlot::empty(1, 2)
        ]
    );
    assert_eq!(opened[1].slots.items, vec![InventorySlot::empty(1, 0)]);
    assert!(
        opened
            .iter()
            .flat_map(|inventory| &inventory.slots.items)
            .all(|slot| slot.item.is_none())
    );
}

#[test]
fn unknown_duplicate_ambiguous_or_missing_bindings_refuse() {
    let catalog = Catalog::new();
    let recipe = recipe();
    let check = |change: &dyn Fn(&mut PlayerInventoryProjectionBinding), expected: &str| {
        let mut changed = binding();
        change(&mut changed);
        let message = refusal(&changed, &recipe, catalog.view(), &[0]);
        assert!(message.contains(expected), "{expected}: {message}");
        assert!(message.starts_with("network.player_inventory_projection"));
    };
    check(&|b| b.format = 3, "format must be 1 or 2");
    check(
        &|b| b.inventories[0].inventory = "pocket".into(),
        "unknown recipe inventory \"pocket\"",
    );
    check(
        &|b| b.inventories[1].inventory = "belt".into(),
        "recipe inventory \"belt\" is bound twice",
    );
    check(
        &|b| {
            b.inventories.remove(0);
        },
        "recipe inventory \"belt\" has no binding",
    );
    check(
        &|b| b.inventories[0].info_row = "Missing".into(),
        "has no D_InventoryInfo row \"Missing\"",
    );
    check(
        &|b| b.inventories[0].info_row = "Twice".into(),
        "lists D_InventoryInfo row \"Twice\" more than once",
    );
    check(
        &|b| b.inventories[0].info_row = "AuthoredBag".into(),
        "D_InventoryInfo row \"AuthoredBag\" is bound twice",
    );
    check(
        &|b| b.inventories[0].info_row = "InfoZero".into(),
        "already opened by the authored D_Inventory Player row",
    );
    check(
        &|b| {
            b.inventories[1].slot_queries.pop();
        },
        "needs one slot query per slot (3)",
    );
    check(
        &|b| b.inventories[1].slot_queries[2] = "Missing".into(),
        "has no D_TagQueries row \"Missing\"",
    );
    check(
        &|b| b.inventories[1].slot_queries[2] = "Twice".into(),
        "lists D_TagQueries row \"Twice\" more than once",
    );
    check(
        &|b| b.items[0].item = "torch".into(),
        "unknown recipe item \"torch\"",
    );
    check(
        &|b| {
            b.items[1] = b.items[0].clone();
        },
        "recipe item \"water\" is bound twice",
    );
    check(
        &|b| {
            b.items.remove(1);
        },
        "recipe item \"ration\" has no binding",
    );
    check(
        &|b| b.items[0].row = "Missing".into(),
        "has no D_ItemsStatic row \"Missing\"",
    );
    check(
        &|b| b.items[0].row = "Twice".into(),
        "lists D_ItemsStatic row \"Twice\" more than once",
    );
    check(
        &|b| b.items[1].row = "AuthoredWater".into(),
        "D_ItemsStatic row \"AuthoredWater\" is bound twice",
    );
    check(
        &|b| b.items[0].count_property = false,
        "recipe item \"water\" stacks above 1 and needs count_property",
    );
    check(
        &|b| b.items[0].dynamic[0].property = 14,
        "dynamic property 14 outside 0..=13",
    );
    check(
        &|b| b.items[0].dynamic[0].property = ITEMABLE_STACK_PROPERTY,
        "sets property 7; use count_property",
    );
    check(
        &|b| {
            b.items[0].dynamic[0] = DynamicProperty {
                property: 13,
                value: 0,
            }
        },
        "links an inventory",
    );
    check(
        &|b| b.items[0].dynamic[1].property = 12,
        "repeats dynamic property 12",
    );
    // Unlinked containers are authored as -1 and accepted.
    let mut linked = binding();
    linked.items[1].dynamic.push(DynamicProperty {
        property: 13,
        value: -1,
    });
    PlayerInventoryProjection::compile(&linked, &recipe, catalog.view(), &[0]).unwrap();
}

#[test]
fn rows_past_their_handles_and_unprojectable_recipes_refuse() {
    // A D_InventoryInfo row past index 255 does not fit its 1-byte handle;
    // item and query rows must stay below the 2-byte "no row" value.
    let mut catalog = Catalog::new();
    catalog.infos = (0..256).map(|row| format!("Info{row}")).collect();
    catalog.infos.push("AuthoredBag".into());
    catalog.infos.push("AuthoredBelt".into());
    let message = refusal(&binding(), &recipe(), catalog.view(), &[]);
    assert!(message.contains("lies past its row handle"), "{message}");
    let mut catalog = Catalog::new();
    catalog.items = (0..u16::MAX).map(|row| format!("Item{row}")).collect();
    catalog.items.push("AuthoredRation".into());
    catalog.items.push("AuthoredWater".into());
    let message = refusal(&binding(), &recipe(), catalog.view(), &[]);
    assert!(message.contains("lies past its row handle"), "{message}");
    let catalog = Catalog::new();
    // A slot count that can grow, or overflow stacks, have no fixed wire slot.
    let mut growing = inventory("bag", 3, &["water", "ration"]);
    growing.initial_slots = 2;
    let message = refusal(
        &binding(),
        &recipe_with(vec![growing, inventory("belt", 1, &["ration"])]),
        catalog.view(),
        &[],
    );
    assert!(message.contains("needs a fixed slot count"), "{message}");
    let mut overflowing = inventory("bag", 3, &["water", "ration"]);
    overflowing.overflow_limit = 1;
    let message = refusal(
        &binding(),
        &recipe_with(vec![overflowing, inventory("belt", 1, &["ration"])]),
        catalog.view(),
        &[],
    );
    assert!(message.contains("needs overflow_limit 0"), "{message}");
}

#[test]
fn bags_project_to_bound_rows_counts_and_authored_dynamic_only() {
    let projection = compiled();
    let slots = projection
        .project(&bag(
            vec![stack(1, "water", 3), None, stack(2, "ration", 1)],
            stack(3, "ration", 1),
        ))
        .unwrap();
    let water = slots[0][0].item.as_ref().unwrap();
    assert_eq!(water.row, 1);
    // Authored pairs plus the count as ItemableStack, ascending.
    assert_eq!(water.dynamic, vec![(0, 0), (7, 3), (12, 900)]);
    assert!(water.stats.is_empty() && water.alterations.is_empty());
    assert_eq!(water.owner_lookup_id, -1);
    assert!(water.guid.units().is_empty(), "stack IDs are never sent");
    assert_eq!(slots[0][0].query_row, 2);
    assert_eq!(slots[0][1], InventorySlot::empty(1, 1));
    let ration = slots[0][2].item.as_ref().unwrap();
    assert_eq!((ration.row, ration.dynamic.as_slice()), (2, &[][..]));
    assert_eq!(slots[1][0].item.as_ref().unwrap().row, 2);
    // A bag that differs from the validated binding is not projected.
    let mut wrong = bag(vec![None, None, None], None);
    wrong[0].slots.pop();
    assert!(projection.project(&wrong).is_err());
    let mut overflow = bag(vec![None, None, None], None);
    overflow[1]
        .overflow
        .push(crate::native_inventory::DeferredStackV1 {
            stack: stack(4, "ration", 1).unwrap(),
            requested_slot: 0,
        });
    assert!(projection.project(&overflow).is_err());
    assert!(
        projection
            .project(&bag(vec![stack(1, "ration", 2), None, None], None))
            .is_err(),
        "an uncounted item never stacks"
    );
}

/// F-978: after the open the client holds key 2 + slots; each set or clear
/// adds one; the delta's base is the key the client holds.
#[test]
fn deltas_follow_the_f978_key_rule_and_split_deterministically() {
    let projection = compiled();
    let mut peer = PeerBagProjection::opened(&projection, 4).unwrap();
    assert_eq!(peer.held()[0].slots.replication_key, 2 + 3);
    assert_eq!(peer.held()[1].slots.replication_key, 2 + 1);
    assert!(!peer.due());
    assert!(peer.deltas(64).unwrap().is_empty());
    assert!(peer.needs_lookup(1));
    // Starter: one stack in the bag's first slot.
    let starter = bag(vec![stack(1, "water", 3), None, None], None);
    assert!(peer.observe(1, 1, || projection.project(&starter)).unwrap());
    assert!(!peer.needs_lookup(1) && peer.needs_lookup(2));
    assert!(peer.due());
    let deltas = peer.deltas(64).unwrap();
    let [delta] = deltas.as_slice() else {
        panic!("one inventory changed");
    };
    assert_eq!(delta.inventory, 4, "first projected subobject");
    assert_eq!(
        (delta.base_key, delta.restored.slots.replication_key),
        (5, 6)
    );
    assert_eq!(delta.changes.len(), 1);
    assert_eq!(delta.changes[0].0, 1, "slot 0 is fast-array ID 1");
    assert_eq!(delta.weight, None);
    peer.commit(deltas);
    assert_eq!(peer.enqueued_revision(), Some(1));
    assert!(!peer.due());
    // The same revision again projects nothing.
    assert!(
        !peer
            .observe(2, 1, || panic!("unchanged revision is not projected"))
            .unwrap()
    );
    // Move to slot 2: one clear and one set, +2.
    let moved = bag(
        vec![None, None, stack(1, "water", 3)],
        stack(2, "ration", 1),
    );
    assert!(peer.observe(3, 2, || projection.project(&moved)).unwrap());
    let deltas = peer.deltas(64).unwrap();
    assert_eq!(deltas.len(), 2);
    assert_eq!(
        (deltas[0].base_key, deltas[0].restored.slots.replication_key),
        (6, 8)
    );
    assert_eq!(
        deltas[0]
            .changes
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!((deltas[1].inventory, deltas[1].base_key), (5, 3));
    assert_eq!(deltas[1].restored.slots.replication_key, 4);
    // At most two changed slots per update: inventory, then slot order.
    let split = peer.deltas(2).unwrap();
    assert_eq!(split.len(), 1);
    assert_eq!(split[0].changes.len(), 2);
    peer.commit(split);
    assert_eq!(peer.enqueued_revision(), Some(1), "partly enqueued");
    assert!(peer.due());
    let rest = peer.deltas(2).unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!((rest[0].inventory, rest[0].base_key), (5, 3));
    peer.commit(rest);
    assert_eq!(peer.enqueued_revision(), Some(2));
    assert_eq!(peer.held()[0].slots.replication_key, 8);
    assert_eq!(peer.held()[1].slots.replication_key, 4);
}

#[test]
fn coalesced_revisions_send_only_the_latest_difference() {
    let projection = compiled();
    let mut peer = PeerBagProjection::opened(&projection, 0).unwrap();
    let first = bag(vec![stack(1, "water", 3), None, None], None);
    peer.observe(1, 1, || projection.project(&first)).unwrap();
    peer.commit(peer.deltas(64).unwrap());
    // Two revisions arrive before the next update: split, then a Use that
    // empties the split-off stack. Only the latest target remains.
    let split = bag(vec![stack(1, "water", 2), stack(2, "water", 1), None], None);
    peer.observe(2, 2, || projection.project(&split)).unwrap();
    let used = bag(vec![stack(1, "water", 2), None, None], None);
    peer.observe(3, 3, || projection.project(&used)).unwrap();
    assert_eq!(peer.target_revision(), Some(3));
    let deltas = peer.deltas(64).unwrap();
    let [delta] = deltas.as_slice() else {
        panic!("one inventory");
    };
    // Slot 1 never differed from what the client holds; only slot 0 is sent.
    assert_eq!(delta.changes.len(), 1);
    assert_eq!(
        (delta.base_key, delta.restored.slots.replication_key),
        (6, 7)
    );
    let SlotChange::Changed(slot) = &delta.changes[0].1 else {
        panic!("a changed slot");
    };
    assert_eq!(
        slot.item.as_ref().unwrap().dynamic,
        vec![(0, 0), (7, 2), (12, 900)]
    );
    // A revision whose slots the client already holds needs no delta.
    peer.commit(deltas);
    let same = bag(vec![stack(1, "water", 2), None, None], None);
    assert!(peer.observe(4, 4, || projection.project(&same)).unwrap());
    assert!(peer.due());
    assert!(peer.deltas(64).unwrap().is_empty());
    peer.commit(Vec::new());
    assert_eq!(peer.enqueued_revision(), Some(4));
}

#[test]
fn recomposed_weighted_pages_use_the_held_slots_and_commit_only_when_enqueued() {
    let projection = PlayerInventoryProjection::compile(
        &weighted_binding(5, 0),
        &recipe(),
        Catalog::new().view(),
        &[0],
    )
    .unwrap();
    let mut peer = PeerBagProjection::opened(&projection, 4).unwrap();
    let starter = bag(vec![stack(1, "water", 3), None, None], None);
    peer.observe(1, 1, || projection.project(&starter)).unwrap();
    peer.commit(peer.deltas(64).unwrap());
    let moved = bag(
        vec![None, None, stack(1, "water", 3)],
        stack(2, "ration", 1),
    );
    peer.observe(2, 2, || projection.project(&moved)).unwrap();
    let before = peer.clone();
    let full = peer.deltas(64).unwrap();
    assert_eq!(
        full.iter().map(|delta| delta.weight).collect::<Vec<_>>(),
        [Some(15), Some(0)]
    );
    let trimmed = peer.deltas(1).unwrap();
    assert_eq!(trimmed[0].weight, Some(0), "only the cleared slot fits");
    assert_eq!(trimmed[0].opened.current_weight, Some(15));
    assert_eq!(
        trimmed,
        peer.deltas(1).unwrap(),
        "a refused enqueue is retryable"
    );
    assert_eq!(peer, before);
    peer.commit(trimmed);
    assert_eq!(peer.enqueued_revision(), Some(1));
    let tail = peer.deltas(64).unwrap();
    assert_eq!(tail[0].opened.current_weight, Some(0));
    assert_eq!(tail[0].weight, Some(15));
    assert_eq!(tail[1].weight, Some(0));
    peer.commit(tail);
    assert_eq!(peer.enqueued_revision(), Some(2));
    assert_eq!(peer.held()[0].current_weight, Some(15));
    assert_eq!(peer.held()[1].current_weight, Some(0));

    let before = peer.clone();
    let oversized = bag(vec![stack(1, "water", 11), None, None], None);
    assert!(
        peer.observe(3, 3, || projection.project(&oversized))
            .is_err()
    );
    assert_eq!(
        peer, before,
        "invalid weight leaves the held and target state intact"
    );
}
