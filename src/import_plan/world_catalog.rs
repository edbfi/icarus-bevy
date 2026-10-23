//! Independently authored structural declarations for dry-run import planning.
//! Membership establishes only that a reference was declared in its context;
//! it does not establish runtime content, gameplay behavior or compatibility.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

/// The `[world]` section of an authored catalog. File size and format version
/// are checked by the parent before compilation.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorldCatalogFile {
    #[serde(default)]
    pub(super) classes: Vec<ClassFile>,
    #[serde(default)]
    pub(super) maps: Vec<MapFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClassFile {
    pub(super) name: String,
    #[serde(default)]
    pub(super) variables: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MapFile {
    pub(super) name: String,
    #[serde(default)]
    pub(super) tiles: Vec<String>,
    #[serde(default)]
    pub(super) actors: Vec<ActorFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActorFile {
    pub(super) path: String,
    pub(super) class: String,
}

/// Exact authored spellings, with variables owned by classes and actors and
/// tiles owned by maps. No original content or runtime world is loaded here.
#[derive(Clone, Default)]
pub(super) struct WorldCatalog {
    classes: BTreeMap<Vec<u16>, BTreeSet<Vec<u16>>>,
    maps: BTreeMap<Vec<u16>, Map>,
}

#[derive(Clone, Default)]
struct Map {
    tiles: BTreeSet<Vec<u16>>,
    actors: BTreeMap<Vec<u16>, Vec<u16>>,
}

impl std::fmt::Debug for WorldCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorldCatalog")
            .field("classes", &self.classes.len())
            .field("maps", &self.maps.len())
            .finish()
    }
}

impl WorldCatalog {
    /// Validate declarations without including supplied strings in errors.
    /// Every index in a diagnostic is zero-based.
    pub(super) fn compile(file: WorldCatalogFile) -> Result<Self> {
        let mut catalog = Self::default();
        for (class_index, class) in file.classes.into_iter().enumerate() {
            let name = reference(&class.name)
                .with_context(|| format!("world catalog class {class_index} name"))?;
            let mut variables = BTreeSet::new();
            for (variable_index, variable) in class.variables.into_iter().enumerate() {
                let variable = reference(&variable).with_context(|| {
                    format!("world catalog class {class_index} variable {variable_index}")
                })?;
                ensure!(
                    variables.insert(variable),
                    "world catalog class {class_index} variable {variable_index} is repeated"
                );
            }
            ensure!(
                catalog.classes.insert(name, variables).is_none(),
                "world catalog class {class_index} name is repeated"
            );
        }

        for (map_index, map) in file.maps.into_iter().enumerate() {
            let name = reference(&map.name)
                .with_context(|| format!("world catalog map {map_index} name"))?;
            let mut compiled = Map::default();
            for (tile_index, tile) in map.tiles.into_iter().enumerate() {
                let tile = reference(&tile)
                    .with_context(|| format!("world catalog map {map_index} tile {tile_index}"))?;
                ensure!(
                    compiled.tiles.insert(tile),
                    "world catalog map {map_index} tile {tile_index} is repeated"
                );
            }
            for (actor_index, actor) in map.actors.into_iter().enumerate() {
                let path = reference(&actor.path).with_context(|| {
                    format!("world catalog map {map_index} actor {actor_index} path")
                })?;
                let class = reference(&actor.class).with_context(|| {
                    format!("world catalog map {map_index} actor {actor_index} class")
                })?;
                ensure!(
                    catalog.classes.contains_key(&class),
                    "world catalog map {map_index} actor {actor_index} class is undeclared"
                );
                ensure!(
                    compiled.actors.insert(path, class).is_none(),
                    "world catalog map {map_index} actor {actor_index} path is repeated"
                );
            }
            ensure!(
                catalog.maps.insert(name, compiled).is_none(),
                "world catalog map {map_index} name is repeated"
            );
        }
        Ok(catalog)
    }

    pub(super) fn contains_map(&self, map: &[u16]) -> bool {
        self.maps.contains_key(map)
    }

    pub(super) fn contains_class(&self, class: &[u16]) -> bool {
        self.classes.contains_key(class)
    }

    pub(super) fn contains_actor(&self, map: &[u16], path: &[u16], class: &[u16]) -> bool {
        self.maps
            .get(map)
            .and_then(|map| map.actors.get(path))
            .is_some_and(|declared_class| declared_class.as_slice() == class)
    }

    pub(super) fn contains_variable(&self, class: &[u16], variable: &[u16]) -> bool {
        self.classes
            .get(class)
            .is_some_and(|variables| variables.contains(variable))
    }

    pub(super) fn contains_tile(&self, map: &[u16], tile: &[u16]) -> bool {
        self.maps
            .get(map)
            .is_some_and(|map| map.tiles.contains(tile))
    }

    /// All underlying spellings, including repetitions across contexts. The
    /// parent deduplicates these together with its other reference sources.
    pub(super) fn references(&self) -> impl Iterator<Item = &[u16]> {
        let classes = self.classes.iter().flat_map(|(name, variables)| {
            std::iter::once(name.as_slice()).chain(variables.iter().map(Vec::as_slice))
        });
        let maps = self.maps.iter().flat_map(|(name, map)| {
            std::iter::once(name.as_slice())
                .chain(map.tiles.iter().map(Vec::as_slice))
                .chain(
                    map.actors
                        .iter()
                        .flat_map(|(path, class)| [path.as_slice(), class.as_slice()]),
                )
        });
        classes.chain(maps)
    }
}

fn reference(value: &str) -> Result<Vec<u16>> {
    // Even a rejected, oversized field allocates at most the reference bound
    // plus one unit. The containing file has its own bound in the parent.
    let units: Vec<u16> = value
        .encode_utf16()
        .take(super::MAX_CATALOG_REFERENCE_UNITS + 1)
        .collect();
    ensure!(
        !units.is_empty() && units.len() <= super::MAX_CATALOG_REFERENCE_UNITS,
        "reference must contain 1..={} UTF-16 units",
        super::MAX_CATALOG_REFERENCE_UNITS
    );
    Ok(units)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTHORED: &str = r#"
[[classes]]
name = 'AuthoredClassA'
variables = ['SharedVariable', 'OnlyVariableA']
[[classes]]
name = 'AuthoredClassB'
variables = ['SharedVariable', 'OnlyVariableB']
[[maps]]
name = 'AuthoredMapA'
tiles = ['SharedTile', 'OnlyTileA']
[[maps.actors]]
path = '/Game/Authored/Shared'
class = 'AuthoredClassA'
[[maps.actors]]
path = '/Game/Authored/OnlyA'
class = 'AuthoredClassA'
[[maps]]
name = 'AuthoredMapB'
tiles = ['SharedTile', 'OnlyTileB']
[[maps.actors]]
path = '/Game/Authored/Shared'
class = 'AuthoredClassB'
"#;

    fn units(value: &str) -> Vec<u16> {
        value.encode_utf16().collect()
    }

    fn authored() -> WorldCatalogFile {
        toml::from_str(AUTHORED).expect("authored declarations parse")
    }

    fn rejected(file: WorldCatalogFile) -> String {
        let error = WorldCatalog::compile(file).expect_err("invalid declarations must fail");
        let diagnostic = format!("{error:#}\n{error:?}");
        for value in [
            "AuthoredClassA",
            "AuthoredClassB",
            "AuthoredMapA",
            "AuthoredMapB",
            "SharedVariable",
            "SharedTile",
            "/Game/Authored/Shared",
        ] {
            assert!(!diagnostic.contains(value), "supplied value leaked");
        }
        diagnostic
    }

    #[test]
    fn resolves_only_within_the_declared_map_and_class() {
        let catalog = WorldCatalog::compile(authored()).unwrap();
        for suffix in ["A", "B"] {
            let map = units(&format!("AuthoredMap{suffix}"));
            let class = units(&format!("AuthoredClass{suffix}"));
            assert!(catalog.contains_map(&map));
            assert!(catalog.contains_class(&class));
            assert!(catalog.contains_actor(&map, &units("/Game/Authored/Shared"), &class));
            assert!(catalog.contains_variable(&class, &units("SharedVariable")));
            assert!(catalog.contains_tile(&map, &units("SharedTile")));
        }
        assert!(!catalog.contains_actor(
            &units("AuthoredMapB"),
            &units("/Game/Authored/OnlyA"),
            &units("AuthoredClassA")
        ));
        assert!(!catalog.contains_actor(
            &units("AuthoredMapA"),
            &units("/Game/Authored/Shared"),
            &units("AuthoredClassB")
        ));
        assert!(!catalog.contains_variable(&units("AuthoredClassB"), &units("OnlyVariableA")));
        assert!(!catalog.contains_variable(&units("AuthoredClassA"), &units("OnlyVariableB")));
        assert!(!catalog.contains_tile(&units("AuthoredMapB"), &units("OnlyTileA")));
        assert!(!catalog.contains_tile(&units("AuthoredMapA"), &units("OnlyTileB")));
        assert!(!catalog.contains_actor(
            &units("MissingMap"),
            &units("/Game/Authored/Shared"),
            &units("AuthoredClassA")
        ));
        assert!(!catalog.contains_variable(&units("MissingClass"), &units("SharedVariable")));
        assert!(!catalog.contains_tile(&units("MissingMap"), &units("SharedTile")));
    }

    #[test]
    fn exact_case_and_roles_are_preserved() {
        let catalog = WorldCatalog::compile(authored()).unwrap();
        assert!(!catalog.contains_class(&units("authoredclassa")));
        assert!(!catalog.contains_map(&units("authoredmapa")));
        for (map, path, class) in [
            ("authoredmapa", "/Game/Authored/Shared", "AuthoredClassA"),
            ("AuthoredMapA", "/game/authored/shared", "AuthoredClassA"),
            ("AuthoredMapA", "/Game/Authored/Shared", "authoredclassa"),
        ] {
            assert!(!catalog.contains_actor(&units(map), &units(path), &units(class)));
        }
        assert!(!catalog.contains_variable(&units("AuthoredClassA"), &units("sharedvariable")));
        assert!(!catalog.contains_variable(&units("authoredclassa"), &units("SharedVariable")));
        assert!(!catalog.contains_tile(&units("AuthoredMapA"), &units("sharedtile")));
        assert!(!catalog.contains_tile(&units("authoredmapa"), &units("SharedTile")));
        assert!(!catalog.contains_map(&units("AuthoredClassA")));
        assert!(!catalog.contains_class(&units("AuthoredMapA")));
    }

    #[test]
    fn case_variants_are_distinct_authored_spellings() {
        let mut file = authored();
        file.classes.push(ClassFile {
            name: "authoredclassa".into(),
            variables: vec!["SharedVariable".into()],
        });
        file.classes[0].variables.push("sharedvariable".into());
        file.maps[0].tiles.push("sharedtile".into());
        file.maps[0].actors.push(ActorFile {
            path: "/game/authored/shared".into(),
            class: "authoredclassa".into(),
        });
        file.maps.push(MapFile {
            name: "authoredmapa".into(),
            tiles: Vec::new(),
            actors: Vec::new(),
        });
        let catalog = WorldCatalog::compile(file).unwrap();
        assert!(catalog.contains_class(&units("authoredclassa")));
        assert!(catalog.contains_map(&units("authoredmapa")));
        assert!(catalog.contains_actor(
            &units("AuthoredMapA"),
            &units("/game/authored/shared"),
            &units("authoredclassa")
        ));
        assert!(catalog.contains_variable(&units("AuthoredClassA"), &units("sharedvariable")));
        assert!(catalog.contains_tile(&units("AuthoredMapA"), &units("sharedtile")));
    }

    #[test]
    fn empty_world_and_omitted_lists_are_valid() {
        let empty = WorldCatalog::compile(toml::from_str("").unwrap()).unwrap();
        assert_eq!(empty.references().count(), 0);
        assert!(!empty.contains_map(&[]));
        assert!(!empty.contains_class(&[]));
        let file =
            toml::from_str("[[classes]]\nname = 'EmptyClass'\n[[maps]]\nname = 'EmptyMap'\n")
                .unwrap();
        let catalog = WorldCatalog::compile(file).unwrap();
        assert!(catalog.contains_map(&units("EmptyMap")));
        assert!(catalog.contains_class(&units("EmptyClass")));
        assert!(!catalog.contains_tile(&units("EmptyMap"), &[]));
        assert!(!catalog.contains_variable(&units("EmptyClass"), &[]));
    }

    #[test]
    fn world_table_nesting_deserializes() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wrapper {
            world: WorldCatalogFile,
        }

        let text = AUTHORED
            .replace("[[classes]]", "[[world.classes]]")
            .replace("[[maps]]", "[[world.maps]]")
            .replace("[[maps.actors]]", "[[world.maps.actors]]");
        let file: Wrapper = toml::from_str(&text).unwrap();
        let catalog = WorldCatalog::compile(file.world).unwrap();
        assert!(catalog.contains_actor(
            &units("AuthoredMapA"),
            &units("/Game/Authored/Shared"),
            &units("AuthoredClassA")
        ));
    }

    #[test]
    fn unknown_fields_and_missing_required_fields_are_rejected() {
        for text in [
            "unexpected = []",
            "[[classes]]\nvariables = []",
            "[[classes]]\nname = 'A'\nunexpected = []",
            "[[maps]]\ntiles = []",
            "[[maps]]\nname = 'A'\nunexpected = []",
            "[[maps]]\nname = 'A'\n[[maps.actors]]\nclass = 'B'",
            "[[maps]]\nname = 'A'\n[[maps.actors]]\npath = '/Game/Authored/A'",
            "[[maps]]\nname = 'A'\n[[maps.actors]]\npath = 'P'\nclass = 'B'\nunexpected = 1",
        ] {
            assert!(toml::from_str::<WorldCatalogFile>(text).is_err());
        }
    }

    #[test]
    fn duplicate_declarations_are_rejected_without_values() {
        let mut file = authored();
        file.classes[1].name = file.classes[0].name.clone();
        assert!(rejected(file).contains("class 1 name is repeated"));

        let mut file = authored();
        file.maps[1].name = file.maps[0].name.clone();
        assert!(rejected(file).contains("map 1 name is repeated"));

        let mut file = authored();
        file.classes[0].variables.push("SharedVariable".into());
        assert!(rejected(file).contains("class 0 variable 2 is repeated"));

        let mut file = authored();
        file.maps[0].tiles.push("SharedTile".into());
        assert!(rejected(file).contains("map 0 tile 2 is repeated"));

        for class in ["AuthoredClassA", "AuthoredClassB"] {
            let mut file = authored();
            file.maps[0].actors.push(ActorFile {
                path: "/Game/Authored/Shared".into(),
                class: class.into(),
            });
            assert!(rejected(file).contains("map 0 actor 2 path is repeated"));
        }
    }

    #[test]
    fn actor_classes_must_be_declared_with_exact_case() {
        for class in ["PrivateMissingClass", "authoredclassa"] {
            let mut file = authored();
            file.maps[0].actors[0].class = class.into();
            let diagnostic = rejected(file);
            assert!(diagnostic.contains("map 0 actor 0 class is undeclared"));
            assert!(!diagnostic.contains(class));
        }
    }

    #[derive(Clone, Copy)]
    enum Field {
        Class,
        Variable,
        Map,
        Tile,
        ActorPath,
        ActorClass,
    }

    const FIELDS: [Field; 6] = [
        Field::Class,
        Field::Variable,
        Field::Map,
        Field::Tile,
        Field::ActorPath,
        Field::ActorClass,
    ];

    fn with_reference(field: Field, value: String) -> WorldCatalogFile {
        let mut file = authored();
        match field {
            Field::Class => {
                for actor in &mut file.maps[0].actors {
                    actor.class = value.clone();
                }
                file.classes[0].name = value;
            }
            Field::Variable => file.classes[0].variables[0] = value,
            Field::Map => file.maps[0].name = value,
            Field::Tile => file.maps[0].tiles[0] = value,
            Field::ActorPath => file.maps[0].actors[0].path = value,
            Field::ActorClass => {
                file.classes.push(ClassFile {
                    name: value.clone(),
                    variables: Vec::new(),
                });
                file.maps[0].actors[0].class = value;
            }
        }
        file
    }

    #[test]
    fn every_string_obeys_utf16_bounds() {
        let limit = super::super::MAX_CATALOG_REFERENCE_UNITS;
        let unicode_at_limit = format!("{}{}", "🦀".repeat(limit / 2), "a".repeat(limit % 2));
        assert_eq!(unicode_at_limit.encode_utf16().count(), limit);
        for field in FIELDS {
            for valid in ["x".into(), "a".repeat(limit), unicode_at_limit.clone()] {
                let catalog = WorldCatalog::compile(with_reference(field, valid.clone())).unwrap();
                assert!(
                    catalog
                        .references()
                        .any(|reference| reference == units(&valid))
                );
            }
            for invalid in [
                String::new(),
                "a".repeat(limit + 1),
                format!("{unicode_at_limit}a"),
            ] {
                let diagnostic = rejected(with_reference(field, invalid));
                assert!(diagnostic.contains("UTF-16 units"));
            }
        }
        // Validate the actor-class field itself, without an invalid declaration
        // causing compilation to stop at an earlier class-name check.
        for invalid in [String::new(), "🦀".repeat(limit / 2 + 1)] {
            let mut file = authored();
            file.maps[0].actors[0].class = invalid;
            let diagnostic = rejected(file);
            assert!(diagnostic.contains("map 0 actor 0 class"));
            assert!(diagnostic.contains("UTF-16 units"));
        }
    }

    #[test]
    fn reference_iteration_and_debug_preserve_privacy_boundaries() {
        let catalog = WorldCatalog::compile(authored()).unwrap();
        let expected: BTreeSet<Vec<u16>> = [
            "AuthoredClassA",
            "AuthoredClassB",
            "AuthoredMapA",
            "AuthoredMapB",
            "SharedVariable",
            "OnlyVariableA",
            "OnlyVariableB",
            "SharedTile",
            "OnlyTileA",
            "OnlyTileB",
            "/Game/Authored/Shared",
            "/Game/Authored/OnlyA",
        ]
        .into_iter()
        .map(units)
        .collect();
        let cloned = catalog.clone();
        let actual: BTreeSet<Vec<u16>> = cloned.references().map(<[u16]>::to_vec).collect();
        assert_eq!(actual, expected);
        assert_eq!(
            format!("{catalog:?}"),
            "WorldCatalog { classes: 2, maps: 2 }"
        );

        for field in FIELDS {
            let diagnostic = rejected(with_reference(field, "PrivateOversized".repeat(1024)));
            assert!(!diagnostic.contains("PrivateOversized"));
        }
    }
}
