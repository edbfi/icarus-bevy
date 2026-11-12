//! Where a joining player's pawn and controller open: a bounded, validated
//! operator input and a per-connection resolver (F-1139, F-1140).
//!
//! The server ships no map data. Positions come only from the operator's own
//! configuration (`[network.spawn]`, the join probe's `--spawn-location`) or,
//! through [`SpawnResolver::resolve`]'s `saved` seam, from the player's own
//! imported save; nothing here reads packaged terrain, collision or gameplay
//! tables. Nothing chooses an altitude: with no configuration and no saved
//! location the opens keep the plan's location, today's origin, unchanged.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::experimental::peer_lab::MOVE_WORLD_BOUND_CM;

/// Largest number of configured spawn points.
pub const MAX_SPAWN_POINTS: usize = 64;
/// Largest whole-centimetre magnitude the open's packed spawn location
/// represents: `quantize_spawn_vector` scales by 10 and needs
/// `|v * 10| < 1 << 24` (`WritePackedVector<10, 24>`, F-571).
pub const SPAWN_WIRE_LIMIT_CM: f64 = 1_677_721.0;
/// Each configured component's bound: the smaller of the move store's world
/// bound (F-1128) and [`SPAWN_WIRE_LIMIT_CM`].
pub const SPAWN_COMPONENT_LIMIT_CM: f64 = MOVE_WORLD_BOUND_CM.min(SPAWN_WIRE_LIMIT_CM);
/// A configured yaw lies within one full turn either way, in degrees.
pub const MAX_SPAWN_YAW_DEGREES: f64 = 360.0;
/// Authored clearance added to a restored saved location's height, so a pawn
/// saved standing on the ground does not open inside it. Not the original's.
pub const SAVED_LOCATION_CLEARANCE_CM: f32 = 50.0;

/// `[network.spawn]`: the operator's spawn points, handed out round-robin by
/// join. Empty (the default) keeps the plan's location.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SpawnConfig {
    pub points: Vec<SpawnPoint>,
}

/// One spawn point: a location in centimetres (Unreal axes, Z up) and an
/// optional yaw in degrees. The yaw is validated and reported but not sent
/// (F-1140).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpawnPoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f64>,
}

impl SpawnConfig {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.points.len() <= MAX_SPAWN_POINTS,
            "network.spawn.points exceeds {MAX_SPAWN_POINTS} points"
        );
        for (index, point) in self.points.iter().enumerate() {
            point
                .validate()
                .map_err(|error| error.context(format!("network.spawn.points[{index}]")))?;
        }
        Ok(())
    }
}

impl SpawnPoint {
    /// Every component finite and within [`SPAWN_COMPONENT_LIMIT_CM`]; a yaw,
    /// when given, finite and within [`MAX_SPAWN_YAW_DEGREES`].
    pub fn validate(&self) -> Result<()> {
        for (axis, value) in [("x", self.x), ("y", self.y), ("z", self.z)] {
            ensure!(value.is_finite(), "spawn {axis} must be finite");
            ensure!(
                value.abs() <= SPAWN_COMPONENT_LIMIT_CM,
                "spawn {axis} must be within ±{SPAWN_COMPONENT_LIMIT_CM} cm"
            );
        }
        if let Some(yaw) = self.yaw {
            ensure!(
                yaw.is_finite() && yaw.abs() <= MAX_SPAWN_YAW_DEGREES,
                "spawn yaw must be finite and within ±{MAX_SPAWN_YAW_DEGREES} degrees"
            );
        }
        Ok(())
    }

    /// The wire location; exact for a validated point up to `f32` rounding,
    /// which keeps it within the bound (the bound is an `f32` value).
    fn location(&self) -> [f32; 3] {
        [self.x, self.y, self.z].map(|value| value as f32)
    }
}

/// Parse `x,y,z` centimetres (the join probe's `--spawn-location`); bounds
/// are [`SpawnPoint::validate`]'s.
pub fn parse_location(text: &str) -> Result<[f64; 3], String> {
    let parts: Vec<&str> = text.split(',').collect();
    let [x, y, z] = parts.as_slice() else {
        return Err(format!(
            "expected x,y,z centimetres, got {} component(s)",
            parts.len()
        ));
    };
    let parse = |part: &str| {
        part.trim()
            .parse::<f64>()
            .map_err(|error| format!("{part:?}: {error}"))
    };
    Ok([parse(x)?, parse(y)?, parse(z)?])
}

/// Where a placement came from, in precedence order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementSource {
    /// The player's restored saved location plus
    /// [`SAVED_LOCATION_CLEARANCE_CM`].
    Saved,
    /// A configured spawn point.
    Configured,
    /// The plan's location (today's origin), as before this input existed.
    Default,
}

/// One connection's resolved placement.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Placement {
    pub location: [f32; 3],
    /// Validated and reported; never sent (F-1140).
    pub yaw: Option<f32>,
    pub source: PlacementSource,
}

/// One resolution and whether a saved location was offered but refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolution {
    pub placement: Placement,
    pub saved_refused: bool,
}

/// Per-connection placement in precedence order: a restored saved
/// location, else the configured points round-robin by join, else the
/// caller's default. Fixed size after construction.
#[derive(Clone, Debug, Default)]
pub struct SpawnResolver {
    points: Vec<SpawnPoint>,
    /// The next configured point; always below `points.len()` when any.
    next: usize,
}

impl SpawnResolver {
    /// Validate `config` and build its resolver.
    pub fn new(config: &SpawnConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            points: config.points.clone(),
            next: 0,
        })
    }

    pub fn point_count(&self) -> usize {
        self.points.len()
    }

    /// Resolve one joining connection. `saved` is the player's restored
    /// saved location when the restore pipeline has one at open time; it is
    /// used raised by [`SAVED_LOCATION_CLEARANCE_CM`] if the result is finite
    /// and within [`SPAWN_COMPONENT_LIMIT_CM`], and otherwise refused (the
    /// configured points or the default follow). A configured point is
    /// consumed only when no saved location is used.
    pub fn resolve(&mut self, saved: Option<[f32; 3]>, default: [f32; 3]) -> Resolution {
        let mut saved_refused = false;
        if let Some([x, y, z]) = saved {
            let raised = [x, y, z + SAVED_LOCATION_CLEARANCE_CM];
            if raised
                .iter()
                .all(|v| v.is_finite() && f64::from(v.abs()) <= SPAWN_COMPONENT_LIMIT_CM)
            {
                return Resolution {
                    placement: Placement {
                        location: raised,
                        yaw: None,
                        source: PlacementSource::Saved,
                    },
                    saved_refused: false,
                };
            }
            saved_refused = true;
        }
        let placement = match self.points.get(self.next) {
            Some(point) => {
                self.next = (self.next + 1) % self.points.len();
                Placement {
                    location: point.location(),
                    yaw: point.yaw.map(|yaw| yaw as f32),
                    source: PlacementSource::Configured,
                }
            }
            None => Placement {
                location: default,
                yaw: None,
                source: PlacementSource::Default,
            },
        };
        Resolution {
            placement,
            saved_refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimental::session_open::quantize_spawn_vector;

    fn point(x: f64, y: f64, z: f64, yaw: Option<f64>) -> SpawnPoint {
        SpawnPoint { x, y, z, yaw }
    }

    #[test]
    fn the_component_bound_is_the_move_bound_inside_the_wire_range() {
        const { assert!(SPAWN_COMPONENT_LIMIT_CM < SPAWN_WIRE_LIMIT_CM) };
        assert_eq!(SPAWN_COMPONENT_LIMIT_CM, 1_048_576.0);
        // The wire range is the encoder's own: the last whole centimetre
        // quantizes either way, the next does not.
        let wire = SPAWN_WIRE_LIMIT_CM as f32;
        assert!(quantize_spawn_vector([wire, -wire, 0.0]).is_ok());
        assert!(quantize_spawn_vector([wire + 1.0, 0.0, 0.0]).is_err());
        assert!(quantize_spawn_vector([0.0, 0.0, -(wire + 1.0)]).is_err());
        // Every accepted point quantizes.
        let limit = SPAWN_COMPONENT_LIMIT_CM;
        let edge = point(limit, -limit, limit, None);
        edge.validate().unwrap();
        assert!(quantize_spawn_vector(edge.location()).is_ok());
    }

    #[test]
    fn validation_refuses_non_finite_out_of_bound_and_bad_yaw() {
        let limit = SPAWN_COMPONENT_LIMIT_CM;
        let above = f64::from_bits(limit.to_bits() + 1);
        point(limit, -limit, 0.0, Some(360.0)).validate().unwrap();
        point(0.0, 0.0, 0.0, Some(-360.0)).validate().unwrap();
        for bad in [
            point(f64::NAN, 0.0, 0.0, None),
            point(0.0, f64::INFINITY, 0.0, None),
            point(0.0, 0.0, f64::NEG_INFINITY, None),
            point(above, 0.0, 0.0, None),
            point(0.0, -above, 0.0, None),
            point(0.0, 0.0, 2.0e6, None),
            point(0.0, 0.0, 0.0, Some(f64::NAN)),
            point(0.0, 0.0, 0.0, Some(360.000_1)),
            point(0.0, 0.0, 0.0, Some(-720.0)),
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
            assert!(
                SpawnConfig { points: vec![bad] }.validate().is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn at_most_64_points() {
        let at = |count| SpawnConfig {
            points: vec![point(1.0, 2.0, 3.0, None); count],
        };
        at(0).validate().unwrap();
        at(MAX_SPAWN_POINTS).validate().unwrap();
        assert!(at(MAX_SPAWN_POINTS + 1).validate().is_err());
        assert!(SpawnResolver::new(&at(MAX_SPAWN_POINTS + 1)).is_err());
    }

    #[test]
    fn locations_parse_three_components() {
        assert_eq!(parse_location("100,-2.5, 3e3"), Ok([100.0, -2.5, 3000.0]));
        for bad in ["", "1,2", "1,2,3,4", "1,,3", "a,b,c", "1;2;3"] {
            assert!(parse_location(bad).is_err(), "{bad}");
        }
        // Parsing accepts any number; validation bounds it.
        let [x, y, z] = parse_location("nan,0,0").unwrap();
        assert!(point(x, y, z, None).validate().is_err());
    }

    #[test]
    fn precedence_is_saved_then_configured_round_robin_then_default() {
        let default = [0.0; 3];
        let mut none = SpawnResolver::new(&SpawnConfig::default()).unwrap();
        for _ in 0..3 {
            let resolved = none.resolve(None, default);
            assert_eq!(
                resolved.placement,
                Placement {
                    location: default,
                    yaw: None,
                    source: PlacementSource::Default
                }
            );
            assert!(!resolved.saved_refused);
        }
        // The caller's default is kept as given.
        assert_eq!(
            none.resolve(None, [1.0, 2.0, 3.0]).placement.location,
            [1.0, 2.0, 3.0]
        );
        let config = SpawnConfig {
            points: vec![
                point(1000.0, -2000.0, 30_000.0, Some(90.0)),
                point(-500.0, 250.0, 40_000.0, None),
                point(7.5, 8.25, 50_000.0, Some(-45.0)),
            ],
        };
        let mut resolver = SpawnResolver::new(&config).unwrap();
        assert_eq!(resolver.point_count(), 3);
        // Seven joins over three points: 0, 1, 2, 0, 1, 2, 0.
        let order: Vec<_> = (0..7)
            .map(|_| resolver.resolve(None, default).placement)
            .collect();
        for (join, placement) in order.iter().enumerate() {
            let expected = config.points[join % 3];
            assert_eq!(placement.source, PlacementSource::Configured);
            assert_eq!(placement.location, expected.location(), "join {join}");
            assert_eq!(placement.yaw, expected.yaw.map(|yaw| yaw as f32));
        }
        // A saved location wins, raised by the clearance, and consumes no point.
        let saved = resolver.resolve(Some([10.0, 20.0, -30.0]), default);
        assert_eq!(
            saved.placement,
            Placement {
                location: [10.0, 20.0, -30.0 + SAVED_LOCATION_CLEARANCE_CM],
                yaw: None,
                source: PlacementSource::Saved
            }
        );
        assert_eq!(
            resolver.resolve(None, default).placement.location,
            config.points[1].location()
        );
        // A saved location that is not finite or leaves the bound once raised
        // is refused; the configured points follow, else the default.
        let top = SPAWN_COMPONENT_LIMIT_CM as f32;
        for bad in [[f32::NAN, 0.0, 0.0], [0.0, 0.0, top], [0.0, -2.0e6, 0.0]] {
            let refused = resolver.resolve(Some(bad), default);
            assert!(refused.saved_refused);
            assert_eq!(refused.placement.source, PlacementSource::Configured);
            let refused = none.resolve(Some(bad), default);
            assert!(refused.saved_refused);
            assert_eq!(refused.placement.source, PlacementSource::Default);
        }
        let edge = none.resolve(Some([0.0, 0.0, top - SAVED_LOCATION_CLEARANCE_CM]), default);
        assert_eq!(edge.placement.source, PlacementSource::Saved);
    }

    #[test]
    fn configuration_parses_and_refuses_unknown_or_malformed_keys() {
        let parsed: SpawnConfig = toml::from_str(
            "points = [{ x = 1000, y = -2000.5, z = 30000 }, { x = 0, y = 0, z = 1, yaw = 90 }]",
        )
        .unwrap();
        assert_eq!(
            parsed.points,
            [
                point(1000.0, -2000.5, 30_000.0, None),
                point(0.0, 0.0, 1.0, Some(90.0))
            ]
        );
        parsed.validate().unwrap();
        assert_eq!(
            toml::from_str::<SpawnConfig>("").unwrap(),
            SpawnConfig::default()
        );
        for bad in [
            "point = []",
            "points = [{ x = 1, y = 2 }]",
            "points = [{ x = 1, y = 2, z = 3, pitch = 4 }]",
            "points = [{ x = \"1\", y = 2, z = 3 }]",
            "points = { x = 1, y = 2, z = 3 }",
        ] {
            assert!(toml::from_str::<SpawnConfig>(bad).is_err(), "{bad}");
        }
        // Parsed but out of bound: refused by validation.
        let far: SpawnConfig = toml::from_str("points = [{ x = 2e6, y = 0, z = 0 }]").unwrap();
        assert!(far.validate().is_err());
    }
}
