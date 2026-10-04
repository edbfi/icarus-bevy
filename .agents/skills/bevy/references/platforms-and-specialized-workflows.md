# Platforms and specialized workflows

These topics extend the same app/version workflow. Consult the exact integration's
upstream manifest and examples before adding dependencies; no third-party plugin
version is certified by this skill's core snippet check.

## Browser builds

Build the application for `wasm32-unknown-unknown` with an intentional feature set.
The 0.19 normal profiles include the default WebGL2 path; select `webgpu` when the
application requires WebGPU and inspect resolved features. Enabling WebGPU is not
evidence of automatic runtime fallback. Consult the
[versioned browser build guide](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/README.md)
for the selected backend and tooling.

Use the repository's existing Trunk, wasm-bindgen, or other pipeline. With direct
wasm-bindgen, match the CLI version to the library resolved in `Cargo.lock`; a
mismatch can prevent glue generation. Build in release mode for representative
performance. Serve the output over HTTP and deploy its assets alongside it; opening
an HTML file directly is not an equivalent test.

Validate canvas sizing, device-pixel ratio, resize, pointer lock, touch, focus,
asset URLs/case/CORS, loading errors, and audio activation. WebGPU requires an
appropriate secure context and browser/device support. A WASM compile alone tests
neither adapter creation nor WGSL validation. Native filesystem APIs and blocking
threads are not portable save/task strategies for the browser. Budget downloads,
memory, texture formats, and first-frame shader compilation for actual devices.

## Voxel content and streaming

Separate authoritative chunk data from mesh/collider derivatives. Use stable
content IDs for persistence and map them to compact runtime palette indices.
Do not persist an enum discriminant or atlas layer index as the block identity.

Track chunk revision, neighbor-boundary dependencies, and a bounded work queue.
An edit on a border can invalidate both chunks. Send immutable snapshots plus a
revision token to meshing tasks, and discard results whose source revision no
longer matches. Coalesce repeat requests, prioritize visible/near work, and budget
main-thread mesh/collider swaps. Keep empty-mesh cleanup and asset lifetime explicit.

Choose naive, culled, or greedy meshing based on shape/material constraints and
measured cost. Merged quads need consistent UV tiling, winding, normals, lighting,
and material boundaries. Atlas filtering/mips can bleed across tiles; texture
arrays or padded atlases require matching shader and asset preparation. KTX2
compression, GPU formats, and array-layer limits must match supported targets.
Validate generation determinism and save IDs independently of the renderer.

## Capture and diagnostics artifacts

For still images use Bevy's screenshot API. For recording, verify the capture
plugin's Bevy line, output configuration, render target, encoder dependencies,
and supported formats. Keep readback/encoding off the frame's critical path where
possible and bound buffering. Define frame pacing, dropped-frame policy, output
completion, and audio synchronization; producing some bytes is not proof of a
valid recording. Check native and browser support separately.

## Porting from another engine

Inventory behaviors, assets, units/axes, ownership, input, update loops, and
platform requirements first. Map a GameObject/Actor/node to entities and
components according to data ownership; move update behavior to schedules rather
than recreating an object inheritance tree inside one component.

Port one playable slice: scene, movement, input, camera, and an interaction.
Validate the content pipeline before scaling the rewrite. glTF transfers supported
geometry/material/animation data, not arbitrary engine scripts, custom shaders,
or prefab semantics. Preserve source asset identifiers in an explicit conversion
map. Check physics units and timing, UI behavior, animation events, and audio
mixing against the original. Engine-specific extraction tools are separate tasks;
this skill does not silently install or execute external asset converters.

Sources: [Bevy examples and platform instructions](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/README.md),
[render API](https://docs.rs/bevy/0.19.1/bevy/render/index.html),
[mesh API](https://docs.rs/bevy/0.19.1/bevy/mesh/index.html),
[task pools](https://docs.rs/bevy/0.19.1/bevy/tasks/index.html).
