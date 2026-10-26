//! Native replication scheduling: determinism, grid/naive equivalence, priority
//! order against direct evaluator calls, budget truncation and domain-ID output.
use std::collections::BTreeSet;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use icarus_bevy::{
    config::ServerConfig,
    experimental::actor_channel::{
        ActorPriorityInputs, ActorPriorityOutcome, evaluate_actor_net_priority,
        round_net_priority_to_int,
    },
    replication::{
        DEFAULT_NET_CULL_DISTANCE_SQUARED, FrameDecisions, FrameDisposition, NetActor,
        NetParameters, NetViewer, RelevancyCandidate, RelevancyGrid, ReplicationConfig,
        ReplicationFrame, ReplicationPlugin, SpatialMode, naive_relevant,
    },
    simulation::{DomainId, Position, WorldSnapshot, build_app},
};

/// xorshift64*: a fixed-seed generator, so every run checks the same inputs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    /// Uniform in `[-1, 1)`.
    fn signed(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 52) as f64 - 1.0
    }

    fn point(&mut self, scale: f64) -> [f32; 3] {
        [
            (self.signed() * scale) as f32,
            (self.signed() * scale) as f32,
            (self.signed() * scale * 0.05) as f32,
        ]
    }
}

fn app(config: ReplicationConfig) -> App {
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1))
        .add_plugins(ReplicationPlugin { config });
    app.finish();
    app.cleanup();
    app
}

fn spawn_actor(app: &mut App, id: u64, location: [f32; 3], params: NetParameters) -> Entity {
    app.world_mut()
        .spawn((
            DomainId(id),
            Position(location),
            NetActor {
                id: DomainId(id),
                params,
            },
        ))
        .id()
}

fn spawn_viewer(app: &mut App, connection: u32, location: [f32; 3], direction: [f32; 3]) {
    app.world_mut().spawn(NetViewer {
        connection,
        location,
        direction,
    });
}

/// Advances to the next replication frame.
fn step(app: &mut App) -> ReplicationFrame {
    let before = app
        .world()
        .resource::<ReplicationFrame>()
        .decisions
        .replication_frame;
    for _ in 0..4 {
        app.update();
        let frame = app.world().resource::<ReplicationFrame>();
        if frame.decisions.replication_frame != before {
            return frame.clone();
        }
    }
    panic!("replication frame did not advance");
}

struct Scene {
    actors: Vec<(u64, [f32; 3])>,
    viewers: Vec<(u32, [f32; 3], [f32; 3])>,
}

fn random_scene(seed: u64, actors: usize, viewers: u32) -> Scene {
    let mut rng = Rng(seed);
    let actors: Vec<(u64, [f32; 3])> = (0..actors)
        .map(|index| (1_000 + index as u64 * 7, rng.point(60_000.0)))
        .collect();
    let viewers = (0..viewers)
        .map(|connection| {
            let near = actors[rng.below(actors.len())].1;
            let offset = rng.point(2_000.0);
            let location = [near[0] + offset[0], near[1] + offset[1], near[2]];
            let angle = rng.signed() * std::f64::consts::PI;
            let direction = [angle.cos() as f32, angle.sin() as f32, 0.0];
            (connection + 1, location, direction)
        })
        .collect();
    Scene { actors, viewers }
}

fn populate(app: &mut App, scene: &Scene, reverse: bool) {
    let mut actors = scene.actors.clone();
    let mut viewers = scene.viewers.clone();
    if reverse {
        actors.reverse();
        viewers.reverse();
    }
    for (id, location) in actors {
        spawn_actor(app, id, location, NetParameters::default());
    }
    for (connection, location, direction) in viewers {
        spawn_viewer(app, connection, location, direction);
    }
}

fn decisions_over(app: &mut App, frames: usize) -> Vec<FrameDecisions> {
    (0..frames).map(|_| step(app).decisions).collect()
}

#[test]
fn same_seed_replays_identical_selections_regardless_of_spawn_order() {
    let scene = random_scene(0x5eed_0001, 400, 3);
    let config = ReplicationConfig {
        connection_budget: 12,
        seed: 42,
        ..Default::default()
    };
    let mut first = app(config);
    populate(&mut first, &scene, false);
    let mut second = app(config);
    populate(&mut second, &scene, true);
    let a = decisions_over(&mut first, 45);
    let b = decisions_over(&mut second, 45);
    assert_eq!(a, b);
    assert!(
        a.iter()
            .all(|frame| frame.disposition == FrameDisposition::Replicated)
    );
    assert!(
        a.iter()
            .flat_map(|frame| &frame.connections)
            .any(|connection| connection.saturated && connection.selected.len() == 12),
        "the scene must exercise budget saturation"
    );
}

#[test]
fn grid_and_naive_stages_make_identical_decisions() {
    let scene = random_scene(0x5eed_0002, 600, 4);
    let naive_config = ReplicationConfig {
        spatial: SpatialMode::Naive,
        connection_budget: 20,
        ..Default::default()
    };
    let grid_config = ReplicationConfig {
        spatial: SpatialMode::Grid,
        ..naive_config
    };
    let mut naive = app(naive_config);
    populate(&mut naive, &scene, false);
    let mut grid = app(grid_config);
    populate(&mut grid, &scene, false);
    let a = decisions_over(&mut naive, 30);
    assert_eq!(a, decisions_over(&mut grid, 30));
    assert!(
        a.iter()
            .flat_map(|frame| &frame.connections)
            .any(|c| c.relevant > 0)
    );
}

/// Coordinates exactly on, and one f32 step either side of, a cull boundary.
fn boundary_point(viewer: [f32; 3], cull: f32, axis: usize, nudge: i32) -> [f32; 3] {
    let mut point = viewer;
    let mut offset = f64::from(cull.max(0.0)).sqrt() as f32;
    for _ in 0..nudge.unsigned_abs() {
        offset = if nudge > 0 {
            offset.next_up()
        } else {
            offset.next_down()
        };
    }
    point[axis] += offset;
    point
}

#[test]
fn grid_matches_naive_relevant_sets_on_random_layouts() {
    let culls = [
        DEFAULT_NET_CULL_DISTANCE_SQUARED,
        1.0,
        0.0,
        -4.0,
        1e-30,
        4.0e6,
        9.0e8,
        f32::INFINITY,
        f32::MAX,
    ];
    let scales = [10.0, 3.0e3, 4.0e4, 2.0e5, 1.0e7, 1.0e13, 1.0e38];
    let mut rng = Rng(0x0a11_ce11_5eed_0003);
    let (mut naive_out, mut grid_out) = (Vec::new(), Vec::new());
    let (mut relevant_pairs, mut boundary_relevant, mut boundary_culled) = (0_usize, 0, 0);
    for layout in 0..3_000 {
        let scale = scales[rng.below(scales.len())];
        let anchor = rng.point(scale);
        let viewers: Vec<[f32; 3]> = (0..1 + rng.below(8))
            .map(|_| match rng.below(4) {
                0 => rng.point(scale),
                1 => anchor,
                2 => {
                    let step = (f64::from(DEFAULT_NET_CULL_DISTANCE_SQUARED).sqrt()
                        * (1.0 + 1.0 / 1024.0)) as f32;
                    [step * rng.below(5) as f32, -step, 0.0]
                }
                _ => [1.0e15, -1.0e15, 0.0],
            })
            .collect();
        let mut candidates = Vec::new();
        for _ in 0..rng.below(64) {
            // A few layouts share one cull so boundary probes are exact.
            let cull = if layout % 3 == 0 {
                DEFAULT_NET_CULL_DISTANCE_SQUARED
            } else {
                culls[rng.below(culls.len())]
            };
            let location = match rng.below(4) {
                0 => rng.point(scale),
                1 => {
                    let offset = rng.point(20_000.0);
                    [
                        anchor[0] + offset[0],
                        anchor[1] + offset[1],
                        anchor[2] + offset[2],
                    ]
                }
                _ => {
                    let viewer = viewers[rng.below(viewers.len())];
                    let nudge = rng.below(5) as i32 - 2;
                    boundary_point(viewer, cull.min(1.0e30), rng.below(3), nudge)
                }
            };
            assert!(location.iter().all(|value| value.is_finite()));
            candidates.push(RelevancyCandidate {
                location,
                net_cull_distance_squared: cull,
            });
        }
        let grid = RelevancyGrid::build(&candidates);
        for &viewer in &viewers {
            naive_relevant(&candidates, viewer, &mut naive_out);
            grid.relevant(&candidates, viewer, &mut grid_out);
            assert_eq!(naive_out, grid_out, "layout {layout}");
            relevant_pairs += naive_out.len();
        }
        if layout % 3 == 0 {
            for _ in 0..4 {
                let viewer = viewers[0];
                let probe = RelevancyCandidate {
                    location: boundary_point(viewer, DEFAULT_NET_CULL_DISTANCE_SQUARED, 0, 0),
                    net_cull_distance_squared: DEFAULT_NET_CULL_DISTANCE_SQUARED,
                };
                let inside = RelevancyCandidate {
                    location: boundary_point(viewer, DEFAULT_NET_CULL_DISTANCE_SQUARED, 0, -2),
                    ..probe
                };
                let probes = [probe, inside];
                let probe_grid = RelevancyGrid::build(&probes);
                naive_relevant(&probes, viewer, &mut naive_out);
                probe_grid.relevant(&probes, viewer, &mut grid_out);
                assert_eq!(naive_out, grid_out);
                boundary_relevant += naive_out.len();
                boundary_culled += 2 - naive_out.len();
            }
        }
    }
    assert!(
        relevant_pairs > 1_000,
        "layouts must contain relevant pairs"
    );
    assert!(boundary_relevant > 0 && boundary_culled > 0);
}

const VIEWER: [f32; 3] = [100.0, 200.0, 0.0];
const FORWARD: [f32; 3] = [1.0, 0.0, 0.0];

/// Front near/far, behind near/far, beside, out of range and a heavier actor.
fn hand_built_scene() -> Vec<(u64, [f32; 3], f32)> {
    let at = |dx: f32, dy: f32| [VIEWER[0] + dx, VIEWER[1] + dy, 0.0];
    vec![
        (11, at(1_000.0, 0.0), 1.0),
        (12, at(5_000.0, 0.0), 1.0),
        (13, at(-600.0, 0.0), 1.0),
        (14, at(-3_000.0, 0.0), 1.0),
        (15, at(0.0, 4_000.0), 1.0),
        (16, at(20_000.0, 0.0), 1.0),
        (17, at(-300.0, 0.0), 3.0),
        (18, at(400.0, 100.0), 1.0),
    ]
}

/// The first frame's order from direct evaluator calls: no channels yet, so
/// the time scale is the spawn-priority default. On a connection's first
/// processed frame every actor is force-relevant (`ForceRelevantFrame 0 >=
/// LastProcessedFrame 0`, F-772), so the actor beyond the cull distance is
/// selected too.
fn expected_first_frame(config: &ReplicationConfig) -> Vec<(DomainId, i32)> {
    let mut expected: Vec<(DomainId, i32)> = hand_built_scene()
        .into_iter()
        .map(|(id, location, net_priority)| {
            let ActorPriorityOutcome::Computed(priority) =
                evaluate_actor_net_priority(&ActorPriorityInputs {
                    b_use_owner_relevancy: false,
                    has_owner: false,
                    is_view_target_or_instigator: false,
                    b_hidden: false,
                    root_location: Some(location),
                    viewer_location: VIEWER,
                    viewer_dir: FORWARD,
                    time: config.spawn_priority_seconds,
                    net_priority,
                })
            else {
                panic!("no owner delegation in this scene");
            };
            (DomainId(id), round_net_priority_to_int(priority))
        })
        .collect();
    expected.sort_by_key(|(id, priority)| (std::cmp::Reverse(*priority), *id));
    expected
}

fn hand_built_app(config: ReplicationConfig) -> App {
    let mut app = app(config);
    for (id, location, net_priority) in hand_built_scene() {
        let params = NetParameters {
            net_priority,
            ..Default::default()
        };
        spawn_actor(&mut app, id, location, params);
    }
    spawn_viewer(&mut app, 7, VIEWER, FORWARD);
    app
}

fn selected(frame: &ReplicationFrame) -> Vec<(DomainId, i32)> {
    frame.decisions.connections[0]
        .selected
        .iter()
        .map(|actor| (actor.id, actor.priority))
        .collect()
}

#[test]
fn priority_order_matches_direct_evaluator_calls() {
    let config = ReplicationConfig::default();
    let mut app = hand_built_app(config);
    let frame = step(&mut app);
    let expected = expected_first_frame(&config);
    assert_eq!(
        expected.len(),
        8,
        "the out-of-range actor is force-relevant"
    );
    let beyond = hand_built_scene()
        .into_iter()
        .filter(|(_, location, _)| {
            let d: f32 = (0..3)
                .map(|axis| (location[axis] - VIEWER[axis]).powi(2))
                .sum();
            d >= DEFAULT_NET_CULL_DISTANCE_SQUARED
        })
        .count();
    assert_eq!(beyond, 1, "one actor is beyond the cull distance");
    let distinct: BTreeSet<i32> = expected.iter().map(|(_, priority)| *priority).collect();
    assert!(
        distinct.len() >= 4,
        "the scene must exercise several priority bands"
    );
    assert_eq!(selected(&frame), expected);
    let connection = &frame.decisions.connections[0];
    assert_eq!(connection.connection, 7);
    assert_eq!((connection.relevant, connection.prioritized), (7, 8));
    assert!(!connection.saturated);
    assert_eq!(frame.decisions.considered, 8);
}

#[test]
fn authored_hidden_changes_priority_but_keeps_colliding_root_relevant_in_both_modes() {
    use icarus_bevy::replication::NetActorFlags;
    let mut histories = Vec::new();
    for spatial in [SpatialMode::Naive, SpatialMode::Grid] {
        for budget in [1, 8] {
            let config = ReplicationConfig {
                spatial,
                connection_budget: budget,
                ..Default::default()
            };
            let mut app = app(config);
            let mut expected = Vec::new();
            for (id, location, hidden) in [
                (1, [1_000.0, 0.0, 0.0], true),
                (2, [1_000.0, 0.0, 0.0], false),
                (3, [-3_000.0, 0.0, 0.0], true),
                (4, [-3_000.0, 0.0, 0.0], false),
            ] {
                let entity = spawn_actor(&mut app, id, location, NetParameters::default());
                app.world_mut().entity_mut(entity).insert(NetActorFlags {
                    hidden,
                    can_be_damaged: true,
                });
                let ActorPriorityOutcome::Computed(priority) =
                    evaluate_actor_net_priority(&ActorPriorityInputs {
                        b_use_owner_relevancy: false,
                        has_owner: false,
                        is_view_target_or_instigator: false,
                        b_hidden: hidden,
                        root_location: Some(location),
                        viewer_location: [0.0; 3],
                        viewer_dir: FORWARD,
                        time: config.spawn_priority_seconds,
                        net_priority: 1.0,
                    })
                else {
                    panic!("computed priority")
                };
                expected.push((DomainId(id), round_net_priority_to_int(priority)));
            }
            expected.sort_by_key(|(id, priority)| (std::cmp::Reverse(*priority), *id));
            assert_eq!(
                expected.iter().map(|(id, _)| id.0).collect::<Vec<_>>(),
                [2, 1, 3, 4]
            );
            spawn_viewer(&mut app, 1, [0.0; 3], FORWARD);
            let first = step(&mut app);
            assert_eq!(selected(&first), expected[..budget.min(4)]);
            assert_eq!(first.decisions.connections[0].relevant, 4);
            let mut history = vec![first.decisions];
            for _ in 0..90 {
                let frame = step(&mut app);
                assert_eq!(
                    frame.decisions.connections[0].relevant,
                    frame.decisions.considered
                );
                assert!(frame.decisions.connections[0].closed.is_empty());
                history.push(frame.decisions);
            }
            histories.push(history);
        }
    }
    assert_eq!(histories[0], histories[2]);
    assert_eq!(histories[1], histories[3]);
}

#[test]
fn budget_truncates_to_the_highest_priorities() {
    for budget in [0, 1, 3, 6] {
        let config = ReplicationConfig {
            connection_budget: budget,
            ..Default::default()
        };
        let mut app = hand_built_app(config);
        let frame = step(&mut app);
        let expected = expected_first_frame(&config);
        assert_eq!(selected(&frame), expected[..budget], "budget {budget}");
        let connection = &frame.decisions.connections[0];
        assert!(connection.saturated, "budget {budget}");
        assert_eq!(connection.open_channels, budget);
    }
}

#[test]
fn starved_actors_win_later_frames_under_a_tight_budget() {
    let config = ReplicationConfig {
        connection_budget: 1,
        ..Default::default()
    };
    let mut app = app(config);
    spawn_actor(&mut app, 2, [500.0, 0.0, 0.0], NetParameters::default());
    spawn_actor(&mut app, 3, [500.0, 0.0, 0.0], NetParameters::default());
    spawn_viewer(&mut app, 1, [0.0, 0.0, 0.0], FORWARD);
    let order: Vec<u64> = (0..6).map(|_| selected(&step(&mut app))[0].0.0).collect();
    assert_eq!(order, [2, 3, 2, 3, 2, 3]);
}

#[test]
fn despawned_actors_lose_their_channels() {
    let mut app = app(ReplicationConfig::default());
    let entities: Vec<Entity> = (1..=3)
        .map(|id| {
            spawn_actor(
                &mut app,
                id,
                [id as f32 * 300.0, 0.0, 0.0],
                NetParameters::default(),
            )
        })
        .collect();
    spawn_viewer(&mut app, 1, [0.0; 3], FORWARD);
    assert_eq!(step(&mut app).decisions.connections[0].open_channels, 3);
    app.world_mut().despawn(entities[1]);
    let frame = step(&mut app);
    let connection = &frame.decisions.connections[0];
    assert_eq!(connection.open_channels, 2);
    assert!(
        connection
            .selected
            .iter()
            .all(|actor| actor.id != DomainId(2))
    );
    assert_eq!(frame.decisions.actors, 2);
}

#[test]
fn frames_without_connections_do_not_replicate() {
    let mut app = app(ReplicationConfig::default());
    spawn_actor(&mut app, 5, [0.0; 3], NetParameters::default());
    app.update();
    app.update();
    let frame = app.world().resource::<ReplicationFrame>();
    assert_eq!(frame.decisions.disposition, FrameDisposition::NoConnections);
    assert_eq!(frame.decisions.replication_frame, 0);
    assert!(frame.decisions.connections.is_empty());
}

#[test]
fn outputs_carry_domain_ids_not_entities() {
    const BASE: u64 = 1 << 40;
    let mut app = app(ReplicationConfig::default());
    let mut domain = BTreeSet::new();
    let mut entities = Vec::new();
    for index in 0..32_u64 {
        let id = BASE + index * 3;
        domain.insert(id);
        let location = [index as f32 * 100.0, 0.0, 0.0];
        entities.push(spawn_actor(
            &mut app,
            id,
            location,
            NetParameters::default(),
        ));
    }
    spawn_viewer(&mut app, 9, [0.0; 3], FORWARD);
    let frame = step(&mut app);
    let json = serde_json::to_value(&frame).unwrap();
    let connections = json["decisions"]["connections"].as_array().unwrap();
    let selected = connections[0]["selected"].as_array().unwrap();
    assert_eq!(selected.len(), 32);
    for actor in selected {
        let keys: BTreeSet<&str> = actor
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, BTreeSet::from(["id", "priority"]));
        let id = actor["id"].as_u64().unwrap();
        assert!(domain.contains(&id));
        assert!(
            entities
                .iter()
                .all(|entity| entity.to_bits() != id && u64::from(entity.index_u32()) != id)
        );
    }
}
