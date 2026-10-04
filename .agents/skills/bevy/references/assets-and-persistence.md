# Assets, scenes, and persistence

## Loading and ownership

`AssetServer::load` returns a handle immediately, not a loaded asset. Retain strong
handles for as long as the content is needed. Look up data in `Assets<T>` only when
available and provide a failure path for missing files and dependencies. Asset
paths are relative to the configured asset source (normally `assets/`), including
case sensitivity on the deployment target.

Check dependency readiness, not just the root's load state. Use
`is_loaded_with_dependencies` or the appropriate `AssetEvent` message when the
feature needs all referenced data. Hot reload can invalidate cached derived data;
respond to the relevant asset modifications instead of loading or allocating a
replacement every frame.

In 0.19 classic reflected world loading lives in `bevy_world_serialization`.
`WorldAssetRoot` instantiates a world asset; `WorldInstanceReady` signals that its
entity hierarchy exists. `bevy_scene` provides BSN scene authoring and templates,
which is a different facility. For glTF, prefer typed `GltfAssetLabel`s when the
glTF feature is enabled; `models/level.glb#Scene0` loads a world asset. A scene's
animation player can be below the root, so wait for readiness and traverse children.
The 0.19 `#Material0` subasset is `GltfMaterial`; `/std` selects its PBR
`StandardMaterial` conversion where enabled.

## Custom loader

Derive `Asset` and `TypePath` for data, implement `AssetLoader` on a `TypePath`
loader, and register both. The example below loads an opaque binary asset without
extra parsing dependencies. It is complete Rust and is type-checked by the verifier.

<!-- verify: presentation -->
```rust
use bevy::{asset::{io::Reader, AssetLoader, LoadContext}, prelude::*};

#[derive(Asset, TypePath)]
struct LevelBytes(Vec<u8>);

#[derive(Default, TypePath)]
struct LevelBytesLoader;

impl AssetLoader for LevelBytesLoader {
    type Asset = LevelBytes;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &Self::Settings,
        _: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(LevelBytes(bytes))
    }

    fn extensions(&self) -> &[&str] { &["levelbin"] }
}

struct LevelAssetPlugin;
impl Plugin for LevelAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<LevelBytes>()
            .init_asset_loader::<LevelBytesLoader>();
    }
}
```

Install this plugin after `AssetPlugin` (provided by normal `DefaultPlugins`).
For structured data, parse and validate format-specific errors before returning
the asset. Load dependencies through `LoadContext::load` or `load_builder`, retain
their handles in the resulting asset, and mark dependency handle fields with
`#[dependency]` for derived dependency visiting. Loading through an unrelated
`AssetServer` does not express the loader's dependency relationship.

Custom `Reader` implementations in 0.19 must implement `seekable`; use streaming
fallbacks when a source cannot seek. Do not assume a Tokio runtime exists inside
a Bevy loader. Expensive decoding and unbounded input sizes deserve explicit
resource limits when the asset source is untrusted.

## Durable saves

Use a versioned save data model when player progress must outlive ECS refactors.
Runtime `Entity`, asset handles, dense palette indices, and query order are not
stable save identifiers. Persist domain IDs and content paths/keys, allocate new
entities during load, then resolve relationships in a second pass.

`DynamicWorld` and reflection are useful for tooling and controlled snapshots,
but do not supply schema migrations or a transactional save service. Decode,
migrate, and validate before replacing active gameplay state. Add old-version and
corruption fixtures for the supported formats. Native saves can use a temporary
file and atomic replacement; browser storage needs an asynchronous backend such
as IndexedDB and handling for quota/commit failures. Report success after storage
commit, not merely after launching a background task.

Sources: [assets](https://docs.rs/bevy/0.19.1/bevy/asset/index.html),
[custom loader example](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/asset/custom_asset.rs),
[world serialization](https://docs.rs/bevy/0.19.1/bevy/world_serialization/index.html),
[animated glTF example](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/animation/animated_mesh.rs).
