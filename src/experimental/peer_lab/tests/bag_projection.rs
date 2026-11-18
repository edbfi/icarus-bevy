//! `network.player_inventory_projection` over loopback in both pawn orders:
//! authored recipe, binding and catalog only; every update is decoded with the
//! repository's readers and compared with the committed bag. No client
//! inventory behaviour is claimed.
use super::*;
use crate::{
    config::ServerConfig,
    experimental::{
        actor_channel::{BlockTarget, SubObjectClass},
        pawn_open::{SlotItem, slot_stream},
        rep_rpc::{
            FAST_ARRAY_COUNT_CAP, FastArrayDelta, FastArrayItem, FastArrayItemLayout,
            read_custom_delta_header, read_fast_array_delta,
        },
    },
    native_inventory::{InventoryDefinitionV1, ItemDefinitionV1, SlotRuleV1},
    native_player_inventory::{
        CompiledPlayerInventoryRecipe, PlayerConsumableV2, PlayerInventoryActionV1,
        PlayerInventoryRecipeV2, PlayerInventoryRequestV1, PlayerInventoryRevisionPairV1,
        PlayerInventoryTransferRequestV1, PlayerSlotEndpointV1, PlayerStackEndpointV1,
        PlayerStarterItemV1,
    },
    native_player_runtime::{
        apply_player_inventory_action, apply_player_inventory_transfer, drain_inventory_activations,
    },
    player_inventory_projection::{
        DynamicProperty, InventoryBinding, ItemBinding, PlayerInventoryProjection,
        PlayerInventoryProjectionBinding, ProjectionCatalog,
    },
    simulation::{
        SNAPSHOT_SCHEMA_PLAYER_INVENTORY, SNAPSHOT_SCHEMA_PLAYER_TRANSFERS, build_app, capture,
    },
};
use bevy::prelude::App;

fn strings(rows: &[&str]) -> Vec<String> {
    rows.iter().map(|row| (*row).to_owned()).collect()
}

/// Water stacks to 10 and is consumable; a ration is single. Starter: three
/// water in the bag's first slot and one ration on the belt.
pub(super) fn bag_recipe() -> CompiledPlayerInventoryRecipe {
    let inventory = |key: &str, slots: u16, allowed: &[&str]| InventoryDefinitionV1 {
        key: key.into(),
        initial_slots: slots,
        slot_rules: (0..slots)
            .map(|_| SlotRuleV1 {
                allowed_items: strings(allowed),
            })
            .collect(),
        force_add_when_zero: false,
        overflow_limit: 0,
    };
    CompiledPlayerInventoryRecipe::compile_versioned(
        PlayerInventoryRecipeV2 {
            format: 2,
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
            inventories: vec![
                inventory("bag", 3, &["water", "ration"]),
                inventory("belt", 1, &["ration"]),
            ],
            starter: vec![
                PlayerStarterItemV1 {
                    inventory: "bag".into(),
                    requested_slot: 0,
                    item: "water".into(),
                    count: 3,
                },
                PlayerStarterItemV1 {
                    inventory: "belt".into(),
                    requested_slot: 0,
                    item: "ration".into(),
                    count: 1,
                },
            ],
            consumables: vec![PlayerConsumableV2 {
                item: "water".into(),
                oxygen: 0,
                water: 25,
                food: 0,
            }],
        }
        .into(),
    )
    .unwrap()
}

/// An authored binding over an authored catalog (rows at known positions).
pub(super) fn projection() -> PlayerInventoryProjection {
    let binding = PlayerInventoryProjectionBinding {
        format: 1,
        inventories: vec![
            InventoryBinding {
                inventory: "bag".into(),
                info_row: "AuthoredBag".into(),
                slot_queries: strings(&["AnyAuthored", "AnyAuthored", "AnyAuthored"]),
            },
            InventoryBinding {
                inventory: "belt".into(),
                info_row: "AuthoredBelt".into(),
                slot_queries: strings(&["RationOnly"]),
            },
        ],
        items: vec![
            ItemBinding {
                item: "water".into(),
                row: "AuthoredWater".into(),
                count_property: true,
                dynamic: vec![DynamicProperty {
                    property: 0,
                    value: 0,
                }],
            },
            ItemBinding {
                item: "ration".into(),
                row: "AuthoredRation".into(),
                count_property: false,
                dynamic: Vec::new(),
            },
        ],
    };
    PlayerInventoryProjection::compile(
        &binding,
        &bag_recipe(),
        ProjectionCatalog {
            items_static: &strings(&["ItemZero", "AuthoredWater", "AuthoredRation"]),
            inventory_info: &strings(&["InfoZero", "AuthoredBag", "AuthoredBelt"]),
            tag_queries: &strings(&["QueryZero", "AnyAuthored", "RationOnly"]),
        },
        &[],
    )
    .unwrap()
}

/// A fresh schema-15 (or, with transfers, schema-16) world for [`context`].
pub(super) fn inventory_world(transfers: bool) -> App {
    let recipe = bag_recipe();
    let state = WorldSnapshot {
        schema_version: if transfers {
            SNAPSHOT_SCHEMA_PLAYER_TRANSFERS
        } else {
            SNAPSHOT_SCHEMA_PLAYER_INVENTORY
        },
        native_players: Some(PlayerSnapshot::empty(context())),
        player_inventory: Some(if transfers {
            recipe.empty_v2().into()
        } else {
            recipe.empty().into()
        }),
        ..Default::default()
    };
    build_app(&ServerConfig::default(), &state).unwrap()
}

/// The checkpoint lab in either pawn order with `run`'s pawn content and
/// the projection enabled.
fn projected_lab(early: bool) -> PeerLab {
    let mut lab = if early { early_lab() } else { lab() };
    production_content(&mut lab);
    lab.enable_player_inventory_projection(projection())
        .unwrap();
    lab
}

/// One `run` bridge interval against `app`'s registry: the checkpoint bridge,
/// then the exclusive inventory activations (`network::drain_player_bridge`).
pub(super) fn bridge(lab: &mut PeerLab, app: &mut App) {
    lab.bridge_player_checkpoints(&mut app.world_mut().resource_mut::<PlayerRegistry>());
    let failed = drain_inventory_activations(app.world_mut());
    lab.finish_inventory_activations(app.world().resource::<PlayerRegistry>(), &failed);
}

/// One joined, identified client with its received bunches and a decoded
/// model of the projected inventories it holds.
pub(super) struct Session {
    pub(super) socket: UdpSocket,
    pub(super) address: SocketAddrV4,
    pub(super) client: PacketSequencer,
    pub(super) connection: u32,
    pub(super) log: Vec<String>,
    pub(super) model: Model,
}

/// The client's projected inventories: key and slot properties each.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Model {
    inventories: Vec<(i32, Vec<BTreeMap<u32, RepValue>>)>,
}

/// One slot's properties as a client holds them; an unsent array is empty.
pub(super) fn slot_properties(slot: &InventorySlot) -> BTreeMap<u32, RepValue> {
    let mut values: BTreeMap<u32, RepValue> = [3, 7, 8]
        .into_iter()
        .map(|handle| {
            (
                handle,
                RepValue::DynamicArray(crate::experimental::rep_layout::RepArray {
                    count: 0,
                    stream: crate::experimental::rep_layout::PropertyStream {
                        entries: Vec::new(),
                        terminator_checksum: None,
                    },
                }),
            )
        })
        .collect();
    for entry in slot_stream(slot).unwrap().entries {
        values.insert(entry.handle, entry.value);
    }
    values
}

impl Model {
    /// After the open: every bound inventory empty at F-978's key (2 + slots).
    fn opened() -> Self {
        Self::opened_with(&projection())
    }

    /// [`Self::opened`] for another binding.
    fn opened_with(projection: &PlayerInventoryProjection) -> Self {
        Self {
            inventories: projection
                .opened_inventories()
                .iter()
                .map(|inventory| {
                    (
                        2 + inventory.slots.items.len() as i32,
                        inventory.slots.items.iter().map(slot_properties).collect(),
                    )
                })
                .collect(),
        }
    }

    /// What a client holding `owner`'s committed bag would hold.
    fn of_bag(app: &App, player_id: u64) -> Vec<Vec<BTreeMap<u32, RepValue>>> {
        Self::of_bag_with(&projection(), app, player_id)
    }

    /// [`Self::of_bag`] for another binding.
    fn of_bag_with(
        projection: &PlayerInventoryProjection,
        app: &App,
        player_id: u64,
    ) -> Vec<Vec<BTreeMap<u32, RepValue>>> {
        let registry = app.world().resource::<PlayerRegistry>();
        let bag = registry.inventory_bag(player_id).unwrap();
        projection
            .project(bag.inventories)
            .unwrap()
            .iter()
            .map(|slots| slots.iter().map(slot_properties).collect())
            .collect()
    }

    fn slots(&self) -> Vec<Vec<BTreeMap<u32, RepValue>>> {
        self.inventories
            .iter()
            .map(|(_, slots)| slots.clone())
            .collect()
    }

    fn keys(&self) -> Vec<i32> {
        self.inventories.iter().map(|(key, _)| *key).collect()
    }
}

/// Every reliable non-open pawn update body among `bunches`, with partial
/// bunches reassembled as `pawn_payload` does (an export-only part carries no
/// body bits); the open is skipped.
pub(super) fn pawn_update_payloads(bunches: &[Bunch]) -> Vec<Payload> {
    let mut payloads = Vec::new();
    let mut partial: Option<(bool, BitWriter)> = None;
    for bunch in bunches
        .iter()
        .filter(|b| b.header.channel_index == PAWN_CHANNEL_INDEX && b.header.reliable)
    {
        let header = &bunch.header;
        if !header.partial {
            if !header.open && !header.has_package_map_exports {
                payloads.push(bunch.payload.clone());
            }
            continue;
        }
        if header.partial_initial {
            partial = Some((header.open, BitWriter::new()));
        }
        let (_, body) = partial.as_mut().expect("a partial bunch follows its first");
        if !header.has_package_map_exports {
            append_bits(body, &bunch.payload);
        }
        if header.partial_final
            && let Some((open, body)) = partial.take()
            && !open
        {
            payloads.push(Payload::from_writer(body));
        }
    }
    payloads
}

/// The slot deltas of every bag update among `bunches` for `connection`'s
/// pawn, by projected inventory (0 = bag, 1 = belt), checking each block's
/// class and that no other subobject is addressed.
pub(super) fn bag_deltas(
    lab: &PeerLab,
    connection: u32,
    bunches: &[Bunch],
) -> Vec<(usize, FastArrayDelta)> {
    bag_deltas_in(lab, connection, 0..2, pawn_update_payloads(bunches))
}

/// [`bag_deltas`] over pawn update bodies already reassembled, for bound
/// inventories at `connection`'s pawn subobjects `subobjects` (the content's
/// own inventories precede them; this file's fixtures open none).
pub(super) fn bag_deltas_in(
    lab: &PeerLab,
    connection: u32,
    subobjects: std::ops::Range<u32>,
    payloads: Vec<Payload>,
) -> Vec<(usize, FastArrayDelta)> {
    let plan = &lab.pawn_open.as_ref().unwrap().0;
    let codec = RepLayoutCodec {
        actor: actor_codec(plan.engine_net_version, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let mut cache = lab.guid_cache.clone();
    let key = pawn_key(connection);
    let guids: Vec<u32> = subobjects
        .map(|index| cache.subobject_guid(key, index).unwrap())
        .collect();
    let mut deltas = Vec::new();
    for payload in payloads {
        let body = read_actor_bunch_body(&mut payload.reader(), false, &codec.actor).unwrap();
        for block in body.blocks {
            let BlockTarget::SubObject { object, class } = &block.target else {
                continue;
            };
            let Some(index) = guids.iter().position(|guid| *guid == object.guid) else {
                // ActorState, or another connection's pawn: not this bag.
                continue;
            };
            assert!(matches!(class, SubObjectClass::Class(_)));
            assert!(!block.has_rep_layout, "no weight is sent");
            let mut reader = block.payload.reader();
            let field =
                read_field_header(&mut reader, plan.content.inventory_bound, 1 << 20).unwrap();
            assert!(reader.at_end());
            assert_eq!(field.index, 4, "UInventory::Slots");
            let mut field_reader = field.payload.reader();
            read_custom_delta_header(&mut field_reader, plan.engine_net_version, 1).unwrap();
            let delta = read_fast_array_delta(
                &mut field_reader,
                &FastArrayItemLayout::Delta(plan.content.slot_layout.clone()),
                &codec,
                FAST_ARRAY_COUNT_CAP,
            )
            .unwrap();
            assert!(field_reader.at_end());
            deltas.push((index, delta));
        }
    }
    deltas
}

impl Session {
    /// Apply decoded deltas, checking F-978's arithmetic: the base is the key
    /// the client holds and the new key adds one per changed slot.
    pub(super) fn apply(&mut self, lab: &PeerLab, bunches: &[Bunch]) -> usize {
        let deltas = bag_deltas(lab, self.connection, bunches);
        for (index, delta) in &deltas {
            let (key, slots) = &mut self.model.inventories[*index];
            assert_eq!(delta.base_replication_key, *key, "base is the held key");
            assert_eq!(
                delta.array_replication_key,
                *key + delta.changed.len() as i32,
                "one key per changed slot"
            );
            assert!(delta.deleted_ids.is_empty());
            *key = delta.array_replication_key;
            for (id, item) in &delta.changed {
                let FastArrayItem::Delta(stream) = item else {
                    panic!("delta items");
                };
                let slot = &mut slots[usize::try_from(id - 1).unwrap()];
                for entry in &stream.entries {
                    slot.insert(entry.handle, entry.value.clone());
                }
            }
        }
        deltas.len()
    }

    /// Bridge and deliver `rounds` times, applying every bag update.
    pub(super) fn settle(&mut self, lab: &mut PeerLab, app: &mut App, rounds: usize) -> usize {
        let mut applied = 0;
        for _ in 0..rounds {
            bridge(lab, app);
            let bunches = deliver_bunches(lab, &self.socket, &mut self.client, |_| true);
            if !bag_deltas(lab, self.connection, &bunches).is_empty() {
                self.log.push("bag".into());
            }
            applied += self.apply(lab, &bunches);
        }
        applied
    }

    fn player_id(&self, lab: &PeerLab) -> u64 {
        lab.peers[&self.address]
            .checkpoint_projection
            .unwrap()
            .player_id
    }

    fn epoch(&self, lab: &PeerLab) -> u64 {
        lab.peers[&self.address].checkpoint_epoch.unwrap()
    }
}

/// Join `account` slot 3 through Begin, profiles, initialisation and the
/// friends list, bridging against `app` until the record is finalised.
pub(super) fn join(lab: &mut PeerLab, app: &mut App, connection: u32, account: u64) -> Session {
    join_with(lab, app, connection, account, Model::opened(), 4)
}

/// [`join`] with the client's opened model and the number of bridge rounds.
fn join_with(
    lab: &mut PeerLab,
    app: &mut App,
    connection: u32,
    account: u64,
    model: Model,
    rounds: usize,
) -> Session {
    let (socket, address, client, opened) = peer(lab, connection, account, true);
    let mut session = Session {
        socket,
        address,
        client,
        connection,
        log: Vec::new(),
        model,
    };
    assert_eq!(session.apply(lab, &opened), 0);
    identify(lab, &session.socket, &mut session.client, address, 3, 3);
    bridge(lab, app);
    let reserved = deliver_bunches(lab, &session.socket, &mut session.client, |_| true);
    assert_eq!(session.apply(lab, &reserved), 0, "no bag before activation");
    lab.controller_call(
        address,
        ControllerCall::Initialisation(InitialisationAnswer::Loadout),
    );
    lab.controller_call(
        address,
        ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
    );
    lab.controller_call(address, ControllerCall::FriendsList);
    for _ in 0..rounds {
        // A new player's Begin re-queries every registered player (F-1051);
        // the other connected clients answer.
        let others: Vec<SocketAddrV4> = lab
            .peers
            .iter()
            .filter(|(other, entry)| {
                **other != address && entry.player.is_some() && !entry.profiles.friend_ids_ready
            })
            .map(|(other, _)| *other)
            .collect();
        for other in others {
            lab.controller_call(other, ControllerCall::FriendsList);
        }
        bridge(lab, app);
        if lab.peers[&address].profiles.finalised {
            session.log.push("finalised".into());
        }
        let bunches = deliver_bunches(lab, &session.socket, &mut session.client, |_| true);
        if session.apply(lab, &bunches) > 0 {
            session.log.push("bag".into());
        }
    }
    assert!(lab.peers[&address].profiles.finalised);
    session.log.dedup();
    session
}

/// One bag action, built from the committed state when it is applied.
type Step<'a> = Box<dyn Fn(&App) -> PlayerInventoryActionV1 + 'a>;

pub(super) fn endpoint(
    app: &App,
    player_id: u64,
    inventory: &str,
    slot: usize,
) -> PlayerStackEndpointV1 {
    let registry = app.world().resource::<PlayerRegistry>();
    let bag = registry.inventory_bag(player_id).unwrap();
    let stack_id = bag
        .inventories
        .iter()
        .find(|i| i.key == inventory)
        .unwrap()
        .slots[slot]
        .as_ref()
        .unwrap()
        .id;
    PlayerStackEndpointV1 {
        owner: bag.owner,
        inventory: inventory.into(),
        stack_id,
    }
}

fn slot_endpoint(app: &App, player_id: u64, inventory: &str, slot: u16) -> PlayerSlotEndpointV1 {
    PlayerSlotEndpointV1 {
        owner: app
            .world()
            .resource::<PlayerRegistry>()
            .inventory_bag(player_id)
            .unwrap()
            .owner,
        inventory: inventory.into(),
        slot,
    }
}

fn revisions(app: &App, player_id: u64) -> PlayerInventoryRevisionPairV1 {
    let registry = app.world().resource::<PlayerRegistry>();
    PlayerInventoryRevisionPairV1 {
        expected_player_revision: registry
            .snapshot()
            .records
            .iter()
            .find(|record| record.id == player_id)
            .unwrap()
            .revision,
        expected_bag_revision: registry.inventory_bag(player_id).unwrap().revision,
    }
}

fn act(app: &mut App, lab: &PeerLab, session: &Session, action: PlayerInventoryActionV1) {
    act_at(app, lab, session.address, action);
}

/// Apply `action` for the slot-3 player of the session at `address`.
pub(super) fn act_at(
    app: &mut App,
    lab: &PeerLab,
    address: SocketAddrV4,
    action: PlayerInventoryActionV1,
) {
    let entry = &lab.peers[&address];
    let player_id = entry.checkpoint_projection.unwrap().player_id;
    let cas = revisions(app, player_id);
    let outcome = apply_player_inventory_action(
        app.world_mut(),
        PlayerKey::new(entry.auth_identity.unwrap(), 3).unwrap(),
        entry.checkpoint_epoch.unwrap(),
        &PlayerInventoryRequestV1 {
            player_id,
            expected_player_revision: cas.expected_player_revision,
            expected_bag_revision: cas.expected_bag_revision,
            action,
        },
    )
    .unwrap();
    assert_eq!(
        outcome,
        crate::native_inventory::InventoryActionOutcomeV1::Applied
    );
}

#[test]
fn enabling_requires_checkpoints_and_the_pawn_before_peers_and_refuses_ambiguity() {
    let mut plain = record_lab(false);
    assert!(
        plain
            .enable_player_inventory_projection(projection())
            .unwrap_err()
            .to_string()
            .contains("requires player checkpoints")
    );
    assert!(!plain.has_player_inventory_projection());
    let mut lab = lab();
    lab.pawn_open = None;
    assert!(
        lab.enable_player_inventory_projection(projection())
            .unwrap_err()
            .to_string()
            .contains("requires the pawn open")
    );
    // An info row the pawn content already opens would be ambiguous.
    let mut lab = super::lab();
    lab.pawn_open.as_mut().unwrap().2.inventories = projection().opened_inventories()[1..].to_vec();
    assert!(
        lab.enable_player_inventory_projection(projection())
            .unwrap_err()
            .to_string()
            .contains("already opened")
    );
    let mut lab = super::lab();
    lab.enable_player_inventory_projection(projection())
        .unwrap();
    assert!(lab.has_player_inventory_projection());
    assert!(
        lab.enable_player_inventory_projection(projection())
            .is_err(),
        "once"
    );
    assert_eq!(
        lab.pawn_open.as_ref().unwrap().2.inventories,
        projection().opened_inventories(),
        "the bound inventories, empty, after the (empty) listed ones"
    );
    assert_eq!(
        lab.session_counts().player_bag_projection,
        Some(Default::default())
    );
    let mut lab = super::lab();
    let (_socket, _address, _client, _) = peer(&mut lab, 1, ACCOUNT, true);
    assert!(
        lab.enable_player_inventory_projection(projection())
            .unwrap_err()
            .to_string()
            .contains("must precede peers")
    );
    assert_eq!(lab.session_counts().player_bag_projection, None);
    assert!(
        !serde_json::to_string(&lab.session_counts())
            .unwrap()
            .contains("player_bag_projection")
    );
}

#[test]
fn off_the_open_and_the_store_are_unchanged_and_no_bag_is_sent() {
    // The same join with and without the projection: the pawn open differs
    // only by the bound inventories; every committed byte is identical.
    let run = |projected: bool| {
        let mut lab = if projected {
            projected_lab(true)
        } else {
            let mut lab = early_lab();
            production_content(&mut lab);
            lab
        };
        let mut app = inventory_world(false);
        let (socket, address, mut client, opened) = peer(&mut lab, 1, ACCOUNT, true);
        identify(&mut lab, &socket, &mut client, address, 3, 3);
        lab.controller_call(
            address,
            ControllerCall::Initialisation(InitialisationAnswer::Loadout),
        );
        lab.controller_call(
            address,
            ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
        );
        lab.controller_call(address, ControllerCall::FriendsList);
        let mut received = Vec::new();
        for _ in 0..4 {
            bridge(&mut lab, &mut app);
            received.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        }
        assert!(lab.peers[&address].profiles.finalised);
        let open: Vec<Bunch> = opened
            .into_iter()
            .filter(|b| b.header.channel_index == PAWN_CHANNEL_INDEX)
            .collect();
        let state = serde_json::to_vec(&capture(app.world_mut()).unwrap()).unwrap();
        (lab, open, received, state)
    };
    let (off, off_open, off_received, off_state) = run(false);
    let (on, on_open, on_received, on_state) = run(true);
    assert_eq!(
        off_state, on_state,
        "the projection never touches the store"
    );
    assert!(bag_deltas(&off, 1, &off_received).is_empty());
    assert!(
        pawn_updates(&off_received).is_empty(),
        "no pawn update at all"
    );
    assert_eq!(bag_deltas(&on, 1, &on_received).len(), 2, "bag and belt");
    // Off, the open lists the content's inventories only (none here).
    let plan = &off.pawn_open.as_ref().unwrap().0;
    let codec = actor_codec(plan.engine_net_version, ByteOrder::Little);
    let subobjects = |bunches: &[Bunch]| {
        read_actor_bunch_body(&mut pawn_payload(bunches).reader(), true, &codec)
            .unwrap()
            .blocks
            .iter()
            .filter(|block| {
                matches!(
                    block.target,
                    BlockTarget::SubObject {
                        class: SubObjectClass::Class(_),
                        ..
                    }
                )
            })
            .count()
    };
    assert_eq!(subobjects(&off_open), 0);
    assert_eq!(subobjects(&on_open), 2);
    assert!(off.pawn_open.as_ref().unwrap().2.inventories.is_empty());
    assert_eq!(off.session_counts().player_bag_projection, None);
}

#[test]
fn the_starter_bag_reaches_a_fresh_player_before_its_record_in_both_pawn_orders() {
    for early in [true, false] {
        let mut lab = projected_lab(early);
        let mut app = inventory_world(false);
        let session = join(&mut lab, &mut app, 1, ACCOUNT);
        // The bag precedes the record's finalisation, as the vitals do.
        assert_eq!(session.log, ["bag", "finalised"], "early {early}");
        let player = session.player_id(&lab);
        assert_eq!(session.model.slots(), Model::of_bag(&app, player));
        // F-978: open 2 + slots, then one per filled slot.
        assert_eq!(session.model.keys(), [2 + 3 + 1, 2 + 1 + 1]);
        let entry = &lab.peers[&session.address];
        let bag = entry.bag_projection.as_ref().unwrap();
        assert_eq!(bag.enqueued_revision(), Some(1));
        let counts = lab.session_counts().player_bag_projection.unwrap();
        assert_eq!(
            (
                counts.revisions_observed,
                counts.updates_queued,
                counts.slots_queued
            ),
            (1, 1, 2)
        );
        assert_eq!(counts.update_failures + counts.projection_errors, 0);
        // Settled: no further bag traffic without a new revision.
        let mut session = session;
        assert_eq!(session.settle(&mut lab, &mut app, 3), 0);
    }
}

#[test]
fn every_bag_operation_projects_the_committed_bag_once() {
    for early in [true, false] {
        let mut lab = projected_lab(early);
        let mut app = inventory_world(false);
        let mut session = join(&mut lab, &mut app, 1, ACCOUNT);
        let player = session.player_id(&lab);
        let water = |app: &App| endpoint(app, player, "bag", 0);
        let steps: Vec<(&str, Step<'_>)> = vec![
            (
                "move",
                Box::new(move |app| PlayerInventoryActionV1::Move {
                    source: water(app),
                    destination: slot_endpoint(app, player, "bag", 2),
                }),
            ),
            (
                "split",
                Box::new(move |app| PlayerInventoryActionV1::Split {
                    source: endpoint(app, player, "bag", 2),
                    destination: slot_endpoint(app, player, "bag", 0),
                    amount: 1,
                }),
            ),
            (
                "merge",
                Box::new(move |app| PlayerInventoryActionV1::Merge {
                    source: endpoint(app, player, "bag", 0),
                    destination: endpoint(app, player, "bag", 2),
                    amount: 1,
                }),
            ),
            (
                "use",
                Box::new(move |app| PlayerInventoryActionV1::Use {
                    source: endpoint(app, player, "bag", 2),
                }),
            ),
        ];
        // The client's slot changes each step: (bag key delta).
        let expected_changes = [2, 2, 2, 1];
        for ((name, action), changes) in steps.iter().zip(expected_changes) {
            let before = session.model.keys();
            let action = action(&app);
            act(&mut app, &lab, &session, action);
            let applied = session.settle(&mut lab, &mut app, 2);
            assert_eq!(applied, 1, "{name}: one bag delta (early {early})");
            assert_eq!(
                session.model.slots(),
                Model::of_bag(&app, player),
                "{name}: decoded equals the bag (early {early})"
            );
            assert_eq!(session.model.keys()[0], before[0] + changes, "{name}");
            assert_eq!(session.model.keys()[1], before[1], "{name}: belt untouched");
        }
        // Use spent one water: the vitals and the bag moved together.
        let current = lab.peers[&session.address]
            .checkpoint_projection
            .unwrap()
            .checkpoint
            .vitals;
        assert_eq!(current.water, vitals().water + 25);
        assert_eq!(session.settle(&mut lab, &mut app, 3), 0, "nothing replays");
    }
}

#[test]
fn transfers_reach_each_owner_with_only_its_own_bag() {
    let mut lab = projected_lab(true);
    let mut app = inventory_world(true);
    let mut first = join(&mut lab, &mut app, 1, ACCOUNT);
    let mut second = join(&mut lab, &mut app, 2, ACCOUNT + 1);
    let (a, b) = (first.player_id(&lab), second.player_id(&lab));
    assert_ne!(a, b);
    assert_eq!(second.model.slots(), Model::of_bag(&app, b));
    let request = PlayerInventoryTransferRequestV1 {
        source: endpoint(&app, a, "bag", 0),
        destination: slot_endpoint(&app, b, "bag", 1),
        amount: 2,
        source_cas: revisions(&app, a),
        destination_cas: revisions(&app, b),
    };
    apply_player_inventory_transfer(
        app.world_mut(),
        PlayerKey::new(ACCOUNT, 3).unwrap(),
        first.epoch(&lab),
        PlayerKey::new(ACCOUNT + 1, 3).unwrap(),
        second.epoch(&lab),
        &request,
    )
    .unwrap();
    bridge(&mut lab, &mut app);
    let to_first = deliver_bunches(&mut lab, &first.socket, &mut first.client, |_| true);
    let to_second = deliver_bunches(&mut lab, &second.socket, &mut second.client, |_| true);
    assert_eq!(first.apply(&lab, &to_first), 1);
    assert_eq!(second.apply(&lab, &to_second), 1);
    assert_eq!(first.model.slots(), Model::of_bag(&app, a));
    assert_eq!(second.model.slots(), Model::of_bag(&app, b));
    // Each connection's updates address only its own pawn's subobjects.
    assert!(bag_deltas(&lab, 2, &to_first).is_empty());
    assert!(bag_deltas(&lab, 1, &to_second).is_empty());
    let water = |model: &Model| model.slots()[0].clone();
    assert_ne!(water(&first.model), water(&second.model));
}

#[test]
fn pressure_and_loss_coalesce_to_the_latest_revision_without_replaying_it() {
    let mut lab = projected_lab(true);
    lab.config.packet.sent_capacity = 16;
    let mut app = inventory_world(false);
    let mut session = join(&mut lab, &mut app, 1, ACCOUNT);
    let player = session.player_id(&lab);
    fill_ring(&mut lab, session.address);
    // Two revisions while nothing can be queued: move, then use.
    let action = PlayerInventoryActionV1::Move {
        source: endpoint(&app, player, "bag", 0),
        destination: slot_endpoint(&app, player, "bag", 1),
    };
    act(&mut app, &lab, &session, action);
    for _ in 0..3 {
        bridge(&mut lab, &mut app);
    }
    let held = lab.peers[&session.address].bag_projection.clone().unwrap();
    assert_eq!(
        held.enqueued_revision(),
        Some(1),
        "nothing queued under pressure"
    );
    let action = PlayerInventoryActionV1::Use {
        source: endpoint(&app, player, "bag", 1),
    };
    act(&mut app, &lab, &session, action);
    let committed = capture(app.world_mut()).unwrap();
    bridge(&mut lab, &mut app);
    assert_eq!(
        lab.peers[&session.address]
            .bag_projection
            .as_ref()
            .unwrap()
            .target_revision(),
        Some(3)
    );
    // Capacity drains; only the latest bag goes, once.
    deliver_bunches(&mut lab, &session.socket, &mut session.client, |_| true);
    bridge(&mut lab, &mut app);
    let lost = deliver_bunches(&mut lab, &session.socket, &mut session.client, |_| false);
    let [lost_update] = pawn_updates(&lost).try_into().unwrap();
    assert_eq!(bag_deltas(&lab, 1, &lost).len(), 1);
    assert_eq!(
        lab.peers[&session.address]
            .bag_projection
            .as_ref()
            .unwrap()
            .enqueued_revision(),
        Some(3)
    );
    for _ in 0..3 {
        bridge(&mut lab, &mut app);
        assert_eq!(capture(app.world_mut()).unwrap(), committed, "read only");
    }
    let datagram = lab
        .peers
        .get_mut(&session.address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&datagram, session.address));
    let repaired = deliver_bunches(&mut lab, &session.socket, &mut session.client, |_| true);
    let [repair] = pawn_updates(&repaired).try_into().unwrap();
    assert_eq!(
        repair.payload, lost_update.payload,
        "the same bunch, resent"
    );
    assert_eq!(session.apply(&lab, &repaired), 1);
    assert_eq!(session.model.slots(), Model::of_bag(&app, player));
    assert_eq!(session.settle(&mut lab, &mut app, 3), 0);
    assert_eq!(capture(app.world_mut()).unwrap(), committed);
    let counts = lab.session_counts().player_bag_projection.unwrap();
    assert_eq!(
        counts.updates_queued, 2,
        "starter, then the coalesced latest"
    );
    assert!(
        counts.update_failures > 0,
        "pressure deferred, never dropped"
    );
}

#[test]
fn a_peer_without_a_bound_checkpoint_receives_no_bag() {
    let mut lab = projected_lab(true);
    let mut app = inventory_world(false);
    let (socket, address, mut client, opened) = peer(&mut lab, 1, ACCOUNT, true);
    assert!(
        lab.peers[&address]
            .peer
            .has_actor_channel(PAWN_CHANNEL_INDEX)
    );
    let mut received = opened;
    for _ in 0..4 {
        bridge(&mut lab, &mut app);
        received.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    }
    assert!(bag_deltas(&lab, 1, &received).is_empty());
    assert!(pawn_updates(&received).is_empty());
    let bag = lab.peers[&address].bag_projection.as_ref().unwrap();
    assert_eq!(bag.target_revision(), None);
    // Identified but never activated (no initialisation): still nothing.
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    let mut identified = Vec::new();
    for _ in 0..4 {
        bridge(&mut lab, &mut app);
        identified.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    }
    assert!(lab.peers[&address].checkpoint_epoch.is_some());
    assert!(!lab.peers[&address].checkpoint_active);
    assert!(bag_deltas(&lab, 1, &identified).is_empty());
    assert_eq!(
        lab.session_counts().player_bag_projection.unwrap(),
        Default::default()
    );
}

#[test]
fn restart_and_rejoin_project_the_retained_bag() {
    for early in [true, false] {
        let mut lab = projected_lab(early);
        let mut app = inventory_world(false);
        let session = join(&mut lab, &mut app, 1, ACCOUNT);
        let player = session.player_id(&lab);
        let action = PlayerInventoryActionV1::Split {
            source: endpoint(&app, player, "bag", 0),
            destination: slot_endpoint(&app, player, "bag", 2),
            amount: 1,
        };
        act(&mut app, &lab, &session, action);
        let retained = Model::of_bag(&app, player);
        lab.remove_peer(session.address);
        bridge(&mut lab, &mut app);
        // Restart: a new process from the committed generation, a new lab.
        let stored = capture(app.world_mut()).unwrap();
        drop(app);
        let mut app = build_app(&ServerConfig::default(), &stored).unwrap();
        let mut lab = projected_lab(early);
        let rejoined = join(&mut lab, &mut app, 2, ACCOUNT);
        assert_eq!(rejoined.player_id(&lab), player);
        assert_eq!(rejoined.model.slots(), retained, "early {early}");
        assert_eq!(rejoined.model.slots(), Model::of_bag(&app, player));
        assert_eq!(rejoined.log, ["bag", "finalised"]);
        // The rejoined client gets the retained bag once, from its fresh open.
        assert_eq!(rejoined.model.keys(), [2 + 3 + 2, 2 + 1 + 1]);
    }
}

/// One slot as the codec sends it from literal authored identities (row
/// indices of the authored catalog in [`projection`]), independent of
/// [`PlayerInventoryProjection::project`].
fn literal_slot(
    query_row: u16,
    index: i32,
    item: Option<(u16, Vec<(u32, i32)>)>,
) -> BTreeMap<u32, RepValue> {
    slot_properties(&InventorySlot {
        query_row,
        index,
        item: item.map(|(row, dynamic)| SlotItem {
            dynamic,
            ..SlotItem::new(row).unwrap()
        }),
    })
}

#[test]
fn the_starter_bag_matches_literal_authored_rows_and_counts() {
    let mut lab = projected_lab(true);
    let mut app = inventory_world(false);
    let mut session = join(&mut lab, &mut app, 1, ACCOUNT);
    // Catalog: AuthoredWater is D_ItemsStatic row 1, AuthoredRation row 2;
    // AnyAuthored is D_TagQueries row 1, RationOnly row 2. The binding's
    // authored dynamic pair (0, 0), then the stack count as property 7.
    assert_eq!(
        session.model.slots(),
        vec![
            vec![
                literal_slot(1, 0, Some((1, vec![(0, 0), (7, 3)]))),
                literal_slot(1, 1, None),
                literal_slot(1, 2, None),
            ],
            vec![literal_slot(2, 0, Some((2, Vec::new())))],
        ]
    );
    let player = session.player_id(&lab);
    let action = PlayerInventoryActionV1::Use {
        source: endpoint(&app, player, "bag", 0),
    };
    act(&mut app, &lab, &session, action);
    assert_eq!(session.settle(&mut lab, &mut app, 2), 1);
    assert_eq!(
        session.model.slots()[0][0],
        literal_slot(1, 0, Some((1, vec![(0, 0), (7, 2)]))),
        "one water used: count 3 to 2"
    );
    assert_eq!(session.model.keys(), [2 + 3 + 1 + 1, 2 + 1 + 1]);
}

/// The bag recipe with a 70-slot bag whose starter fills every slot.
fn wide_recipe() -> CompiledPlayerInventoryRecipe {
    let inventory = |key: &str, slots: u16, allowed: &[&str]| InventoryDefinitionV1 {
        key: key.into(),
        initial_slots: slots,
        slot_rules: (0..slots)
            .map(|_| SlotRuleV1 {
                allowed_items: strings(allowed),
            })
            .collect(),
        force_add_when_zero: false,
        overflow_limit: 0,
    };
    let mut starter: Vec<_> = (0..WIDE_SLOTS)
        .map(|slot| PlayerStarterItemV1 {
            inventory: "bag".into(),
            requested_slot: slot,
            item: "water".into(),
            count: 1 + u32::from(slot % 5),
        })
        .collect();
    starter.push(PlayerStarterItemV1 {
        inventory: "belt".into(),
        requested_slot: 0,
        item: "ration".into(),
        count: 1,
    });
    CompiledPlayerInventoryRecipe::compile_versioned(
        PlayerInventoryRecipeV2 {
            format: 2,
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
            inventories: vec![
                inventory("bag", WIDE_SLOTS, &["water", "ration"]),
                inventory("belt", 1, &["ration"]),
            ],
            starter,
            consumables: vec![PlayerConsumableV2 {
                item: "water".into(),
                oxygen: 0,
                water: 25,
                food: 0,
            }],
        }
        .into(),
    )
    .unwrap()
}

const WIDE_SLOTS: u16 = 70;

fn wide_projection() -> PlayerInventoryProjection {
    let binding = PlayerInventoryProjectionBinding {
        format: 1,
        inventories: vec![
            InventoryBinding {
                inventory: "bag".into(),
                info_row: "AuthoredBag".into(),
                slot_queries: vec!["AnyAuthored".to_owned(); usize::from(WIDE_SLOTS)],
            },
            InventoryBinding {
                inventory: "belt".into(),
                info_row: "AuthoredBelt".into(),
                slot_queries: strings(&["RationOnly"]),
            },
        ],
        items: vec![
            ItemBinding {
                item: "water".into(),
                row: "AuthoredWater".into(),
                count_property: true,
                dynamic: Vec::new(),
            },
            ItemBinding {
                item: "ration".into(),
                row: "AuthoredRation".into(),
                count_property: false,
                dynamic: Vec::new(),
            },
        ],
    };
    PlayerInventoryProjection::compile(
        &binding,
        &wide_recipe(),
        ProjectionCatalog {
            items_static: &strings(&["ItemZero", "AuthoredWater", "AuthoredRation"]),
            inventory_info: &strings(&["InfoZero", "AuthoredBag", "AuthoredBelt"]),
            tag_queries: &strings(&["QueryZero", "AnyAuthored", "RationOnly"]),
        },
        &[],
    )
    .unwrap()
}

#[test]
fn a_change_over_64_slots_splits_deterministically_with_chained_keys() {
    for early in [true, false] {
        let mut lab = if early { early_lab() } else { lab() };
        production_content(&mut lab);
        lab.enable_player_inventory_projection(wide_projection())
            .unwrap();
        let recipe = wide_recipe();
        let mut app = build_app(
            &ServerConfig::default(),
            &WorldSnapshot {
                schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
                native_players: Some(PlayerSnapshot::empty(context())),
                player_inventory: Some(recipe.empty().into()),
                ..Default::default()
            },
        )
        .unwrap();
        let session = join_with(
            &mut lab,
            &mut app,
            1,
            ACCOUNT,
            Model::opened_with(&wide_projection()),
            8,
        );
        assert_eq!(session.log, ["bag", "finalised"], "early {early}");
        let player = session.player_id(&lab);
        // 71 changed slots: 64 of the bag, then its last 6 with the belt's 1.
        let counts = lab.session_counts().player_bag_projection.unwrap();
        assert_eq!(
            (counts.updates_queued, counts.slots_queued),
            (2, 71),
            "early {early}"
        );
        // Each delta's base was the held key and its key added its changed
        // slots (`Session::apply`); across the split the keys chain:
        // bag 2 + 70 opened, + 64, + 6; belt 2 + 1 opened, + 1.
        assert_eq!(session.model.keys(), [2 + 70 + 64 + 6, 2 + 1 + 1]);
        assert_eq!(
            session.model.slots(),
            Model::of_bag_with(&wide_projection(), &app, player)
        );
        // Literal authored rows: water is row 1, counts 1..=5 by slot.
        for (slot, values) in session.model.slots()[0].iter().enumerate() {
            let count = 1 + (slot % 5) as i32;
            assert_eq!(
                *values,
                literal_slot(1, slot as i32, Some((1, vec![(7, count)]))),
                "slot {slot}"
            );
        }
        assert_eq!(
            lab.peers[&session.address]
                .bag_projection
                .as_ref()
                .unwrap()
                .enqueued_revision(),
            Some(1)
        );
    }
}

#[test]
fn the_split_is_the_same_every_time() {
    // Determinism: the same bag splits into the same updates.
    let run = || {
        let mut lab = early_lab();
        production_content(&mut lab);
        lab.enable_player_inventory_projection(wide_projection())
            .unwrap();
        let mut app = build_app(
            &ServerConfig::default(),
            &WorldSnapshot {
                schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
                native_players: Some(PlayerSnapshot::empty(context())),
                player_inventory: Some(wide_recipe().empty().into()),
                ..Default::default()
            },
        )
        .unwrap();
        let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
        identify(&mut lab, &socket, &mut client, address, 3, 3);
        lab.controller_call(
            address,
            ControllerCall::Initialisation(InitialisationAnswer::Loadout),
        );
        lab.controller_call(
            address,
            ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
        );
        lab.controller_call(address, ControllerCall::FriendsList);
        let mut received = Vec::new();
        for _ in 0..8 {
            bridge(&mut lab, &mut app);
            received.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        }
        bag_deltas(&lab, 1, &received)
            .into_iter()
            .map(|(index, delta)| {
                (
                    index,
                    delta.base_replication_key,
                    delta.array_replication_key,
                    delta.changed.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    let first = run();
    assert_eq!(first, run());
    let ids = |range: std::ops::RangeInclusive<i32>| range.collect::<Vec<_>>();
    assert_eq!(
        first,
        [
            (0, 72, 136, ids(1..=64)),
            (0, 136, 142, ids(65..=70)),
            (1, 3, 4, vec![1]),
        ]
    );
}

#[test]
fn a_disconnect_with_a_bag_update_in_flight_rejoins_from_a_fresh_baseline() {
    for early in [true, false] {
        let mut lab = projected_lab(early);
        let mut app = inventory_world(false);
        let session = join(&mut lab, &mut app, 1, ACCOUNT);
        let player = session.player_id(&lab);
        let epoch = session.epoch(&lab);
        let action = PlayerInventoryActionV1::Move {
            source: endpoint(&app, player, "bag", 0),
            destination: slot_endpoint(&app, player, "bag", 2),
        };
        act(&mut app, &lab, &session, action);
        bridge(&mut lab, &mut app);
        // Queued but never acknowledged: the update is in flight.
        let entry = &lab.peers[&session.address];
        assert_eq!(
            entry.bag_projection.as_ref().unwrap().enqueued_revision(),
            Some(2)
        );
        assert!(!entry.peer.actor_channel_settled(PAWN_CHANNEL_INDEX));
        let committed = capture(app.world_mut()).unwrap();
        // The client goes silent until the idle timeout.
        let timeout = lab.config.idle_timeout;
        lab.peers.get_mut(&session.address).unwrap().last_seen =
            Instant::now() - timeout - Duration::from_secs(1);
        lab.poll();
        assert!(!lab.peers.contains_key(&session.address));
        assert_eq!(lab.stats.peers_expired, 1);
        bridge(&mut lab, &mut app);
        let registry = app.world().resource::<PlayerRegistry>();
        assert!(registry.projection(epoch).is_none(), "epoch detached");
        assert!(registry.active_epochs().is_empty());
        assert_eq!(capture(app.world_mut()).unwrap(), committed, "read only");
        let counts = lab.session_counts().player_bag_projection.unwrap();
        assert_eq!(counts.sessions_closed, 0, "a disconnect is no bag failure");
        // The same player rejoins on a new connection: the whole current bag
        // from the fresh open's keys, not from the lost update's.
        let rejoined = join(&mut lab, &mut app, 2, ACCOUNT);
        assert_eq!(rejoined.player_id(&lab), player);
        assert_eq!(rejoined.log, ["bag", "finalised"], "early {early}");
        assert_eq!(rejoined.model.slots(), Model::of_bag(&app, player));
        assert_eq!(
            rejoined.model.slots()[0],
            [
                literal_slot(1, 0, None),
                literal_slot(1, 1, None),
                literal_slot(1, 2, Some((1, vec![(0, 0), (7, 3)]))),
            ]
        );
        assert_eq!(rejoined.model.keys(), [2 + 3 + 1, 2 + 1 + 1]);
    }
}

#[test]
fn a_bag_that_can_never_be_sent_closes_only_its_session() {
    for early in [true, false] {
        let mut lab = if early { early_lab() } else { lab() };
        production_content(&mut lab);
        lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
            .unwrap();
        lab.enable_player_inventory_projection(projection())
            .unwrap();
        let mut app = inventory_world(false);
        let first = join(&mut lab, &mut app, 1, ACCOUNT);
        let mut second = join(&mut lab, &mut app, 2, ACCOUNT + 1);
        let (a, b) = (first.player_id(&lab), second.player_id(&lab));
        let first_epoch = first.epoch(&lab);
        let first_uid = lab.peers[&first.address].pawn_uid.unwrap();
        assert_eq!(lab.claimed_actor_uids().len(), 2);
        // Crafted state: the first client is taken to hold its bag at the
        // largest key, so no delta's key can follow it (F-978).
        lab.peers
            .get_mut(&first.address)
            .unwrap()
            .bag_projection
            .as_mut()
            .unwrap()
            .force_held_key(0, i32::MAX);
        let failed_before = lab.stats.peers_failed;
        let profile_failed_before = lab.stats.profile_players_failed;
        for (session, player) in [(&first, a), (&second, b)] {
            let action = PlayerInventoryActionV1::Move {
                source: endpoint(&app, player, "bag", 0),
                destination: slot_endpoint(&app, player, "bag", 1),
            };
            act(&mut app, &lab, session, action);
        }
        let committed = capture(app.world_mut()).unwrap();
        bridge(&mut lab, &mut app);
        assert!(!lab.peers.contains_key(&first.address), "early {early}");
        assert!(!lab.claimed_actor_uids().contains(&first_uid), "UID freed");
        assert_eq!(lab.claimed_actor_uids().len(), 1);
        assert!(
            app.world()
                .resource::<PlayerRegistry>()
                .projection(first_epoch)
                .is_none(),
            "epoch detached at once"
        );
        assert_eq!(lab.stats.peers_failed, failed_before + 1);
        // A bag failure is not a profile-exchange failure.
        assert_eq!(lab.stats.profile_players_failed, profile_failed_before);
        let counts = lab.session_counts().player_bag_projection.unwrap();
        assert_eq!((counts.sessions_closed, counts.projection_errors), (1, 1));
        // The other player is unaffected and the store is unchanged.
        let bunches = deliver_bunches(&mut lab, &second.socket, &mut second.client, |_| true);
        assert_eq!(second.apply(&lab, &bunches), 1);
        assert_eq!(second.model.slots(), Model::of_bag(&app, b));
        // Never retried.
        assert_eq!(second.settle(&mut lab, &mut app, 3), 0);
        let after = lab.session_counts().player_bag_projection.unwrap();
        assert_eq!((after.sessions_closed, after.projection_errors), (1, 1));
        assert_eq!(capture(app.world_mut()).unwrap(), committed);
    }
}

#[test]
fn projection_and_returning_pawn_inputs_refuse_each_other_in_either_order() {
    use crate::experimental::{
        inventory_restore::InventoryRestoreTables,
        recorders::{SavedPlayerCharacter, SavedPlayerController, SavedPlayerState},
        stat_restore::StatRestoreTables,
    };
    type Enable = fn(&mut PeerLab) -> anyhow::Result<()>;
    let inputs: [(&str, Enable); 5] = [
        ("returning characters", |lab| {
            lab.enable_returning_characters(vec![SavedPlayerCharacter {
                player_id: "AuthoredPlayer".encode_utf16().collect(),
                chr_slot: 3,
                icarus_uid: 70_001,
                state: SavedPlayerState::default(),
                inventories: Vec::new(),
                modifiers: Vec::new(),
            }])
        }),
        ("saved controllers", |lab| {
            lab.enable_saved_player_controllers(vec![SavedPlayerController {
                player_id: "AuthoredPlayer".encode_utf16().collect(),
                chr_slot: 3,
            }])
        }),
        ("linked inventories", |lab| {
            lab.set_linked_inventories(BTreeMap::new())
        }),
        ("inventory restore", |lab| {
            lab.enable_inventory_restore(InventoryRestoreTables::default())
        }),
        ("stat restore", |lab| {
            lab.enable_stat_restore(StatRestoreTables::default())
        }),
    ];
    let fresh_only = "serves fresh checkpoint players only";
    for (name, enable) in inputs {
        for early in [true, false] {
            // Projection first: the returning-pawn input refuses, unset.
            let mut lab = if early { early_lab() } else { lab() };
            lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
                .unwrap();
            lab.enable_player_inventory_projection(projection())
                .unwrap();
            let error = enable(&mut lab).unwrap_err().to_string();
            assert!(error.contains(fresh_only), "{name}: {error}");
            assert!(
                lab.returning_characters.is_empty()
                    && lab.saved_player_controllers.is_empty()
                    && lab.linked_inventories.is_none()
                    && lab.inventory_restore.is_none()
                    && lab.stat_restore.is_none(),
                "{name}"
            );
            assert!(lab.has_player_inventory_projection());
            // Input first: the projection refuses and leaves the pawn content.
            let mut lab = if early { early_lab() } else { super::lab() };
            lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
                .unwrap();
            enable(&mut lab).unwrap();
            let listed = lab.pawn_open.as_ref().unwrap().2.inventories.clone();
            let error = lab
                .enable_player_inventory_projection(projection())
                .unwrap_err()
                .to_string();
            assert!(error.contains(fresh_only), "{name}: {error}");
            assert!(!lab.has_player_inventory_projection(), "{name}");
            assert_eq!(lab.pawn_open.as_ref().unwrap().2.inventories, listed);
        }
    }
}

/// `network.checkpoint_saved_locations` over a schema-15 bag world in both
/// pawn orders: a save commits through the full-generation preflight; the
/// location-only revision leaves the bag, its revision and its projection
/// alone and sends neither a bag nor a vitals update.
#[test]
fn saved_locations_preflight_the_bag_world_and_project_nothing() {
    use super::super::move_acks::authored_move;
    for early in [true, false] {
        let mut lab = projected_lab(early);
        lab.enable_pawn_move_acks().unwrap();
        lab.enable_checkpoint_saved_locations().unwrap();
        let mut app = inventory_world(false);
        let mut session = join(&mut lab, &mut app, 1, ACCOUNT);
        let player = session.player_id(&lab);
        let key = PlayerKey::new(ACCOUNT, 3).unwrap();
        let projection = |app: &App| {
            app.world()
                .resource::<PlayerRegistry>()
                .snapshot()
                .projection(key)
                .unwrap()
        };
        let bag_revision = |app: &App| {
            app.world()
                .resource::<PlayerRegistry>()
                .inventory_bag(player)
                .unwrap()
                .revision
        };
        let before = projection(&app);
        let bag_before = bag_revision(&app);
        let queued = lab.stats.player_bag_projection;
        let moved = super::move_now(
            &mut lab,
            &session.socket,
            &mut session.client,
            Some(authored_move(4.0, [110.0, 200.0, 300.0], Some(1))),
            Instant::now(),
        );
        assert_eq!(session.apply(&lab, &moved), 0);
        lab.flush_saved_locations(app.world_mut());
        let saved = projection(&app);
        assert_eq!(
            saved.checkpoint.location_cm,
            [110.0, 200.0, 300.0],
            "early {early}"
        );
        assert_eq!(saved.checkpoint.vitals, before.checkpoint.vitals);
        assert_eq!(saved.revision, before.revision + 1);
        assert_eq!(bag_revision(&app), bag_before);
        // The committed generation is capturable and holds the location.
        let generation = capture(app.world_mut()).unwrap();
        assert_eq!(
            generation
                .native_players
                .unwrap()
                .projection(key)
                .unwrap()
                .checkpoint
                .location_cm,
            [110.0, 200.0, 300.0]
        );
        bridge(&mut lab, &mut app);
        let after = deliver_bunches(&mut lab, &session.socket, &mut session.client, |_| true);
        assert_eq!(session.apply(&lab, &after), 0);
        assert!(pawn_update_payloads(&after).is_empty(), "early {early}");
        assert_eq!(lab.stats.player_bag_projection, queued);
        assert_eq!(session.model.slots(), Model::of_bag(&app, player));
        assert_eq!(
            lab.session_counts()
                .checkpoint_saved_locations
                .unwrap()
                .saved,
            1
        );
    }
}

/// On a schema 15 or 16 world, a location batch whose full-generation step
/// fails returns an error and leaves the registry (players and bags) and the
/// store exactly as before, including a batch mixing a valid save with an
/// invalid one. The failure is injected as a domain entity with a non-finite
/// position, which the generation's validation refuses: a location-only edit
/// cannot fail it, as the edit rules check the same checkpoint bounds first.
#[test]
fn a_failed_location_preflight_changes_neither_registry_nor_store() {
    use crate::{
        native_player_runtime::{LocationSave, LocationSaveOutcome, save_player_locations},
        persistence::SnapshotStore,
        simulation::{DomainId, Position},
    };
    for transfers in [false, true] {
        let mut lab = projected_lab(true);
        let mut app = inventory_world(transfers);
        let session = join(&mut lab, &mut app, 1, ACCOUNT);
        let epoch = lab.peers[&session.address].checkpoint_epoch.unwrap();
        let key = PlayerKey::new(ACCOUNT, 3).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save(&capture(app.world_mut()).unwrap()).unwrap();
        let stored = store.load().unwrap().unwrap();
        let registry = |app: &App| {
            let registry = app.world().resource::<PlayerRegistry>();
            (registry.snapshot(), registry.inventory_snapshot())
        };
        let before = registry(&app);
        let valid = LocationSave {
            key,
            epoch,
            location_cm: [110.0, 200.0, 300.0],
        };
        let invalid = LocationSave {
            key: PlayerKey::new(ACCOUNT + 1, 3).unwrap(),
            epoch: 999,
            location_cm: [f64::NAN; 3],
        };
        let bad = app
            .world_mut()
            .spawn((DomainId(77), Position([f32::NAN, 0.0, 0.0])))
            .id();
        for batch in [vec![valid], vec![valid, invalid], vec![invalid, valid]] {
            assert!(
                save_player_locations(app.world_mut(), &batch).is_err(),
                "transfers {transfers}"
            );
            assert!(registry(&app) == before, "transfers {transfers}");
            let reloaded = store.load().unwrap().unwrap();
            assert_eq!(reloaded.snapshot, stored.snapshot);
            assert_eq!(reloaded.selected_generation, stored.selected_generation);
        }
        // Without the defect, the same mixed batch commits only the valid save.
        app.world_mut().despawn(bad);
        assert_eq!(
            save_player_locations(app.world_mut(), &[invalid, valid]).unwrap(),
            [LocationSaveOutcome::Refused, LocationSaveOutcome::Saved]
        );
        let (players, bags) = registry(&app);
        assert_eq!(
            players.projection(key).unwrap().checkpoint.location_cm,
            [110.0, 200.0, 300.0]
        );
        assert!(bags == before.1);
    }
}
