# Earshot

Physically based acoustics for Minecraft 26.3: occlusion, sound bending around corners, raytraced reverb and echo, and binaural (HRTF) audio. It applies to the game's own sounds and, through [Yak VC](https://github.com/desocketed/yakvc)'s voice sink ([desocketed/yakvc#174](https://github.com/desocketed/yakvc/issues/174)), to proximity voice. It is a client-only Fabric mod.

Status: **M2**: game sounds are muffled by what is in the way, come round corners from the opening they pass through, fade with air absorption, and are muffled underwater. Reverb is next (M3). [docs/PLAN.md](docs/PLAN.md) has the design, the milestones and the measurements so far.

## How it works

- **Simulation (Rust).** A Rust engine, loaded through Java's Foreign Function & Memory API, keeps a voxel copy of the blocks around you. It traces rays through them to work out occlusion, the paths sound takes around walls, and the reverb of the space you're in.
- **Rendering (OpenAL Soft).** The OpenAL Soft that ships with Minecraft renders the result:
  - EFX filters for muffling;
  - EAX or convolution reverb, with impulse responses synthesized from the raytrace;
  - HRTF for direction.

## Development

You need JDK 25 and rustup. rustup reads `rust-toolchain.toml`. Linux cross builds also need `cargo-zigbuild`.

```sh
cargo test --workspace          # Rust crates
cargo xtask header              # regenerate crates/earshot-ffi/include/earshot.h after changing the C ABI
cd mod && ./gradlew runClient   # builds the native library for this machine and starts the game
cd mod && ./gradlew alBench     # what OpenAL Soft costs with Earshot's effects (M0)
cargo xtask dist                # the mod jar in dist/, with this machine's native library only
```

Building in a VM, from a folder shared with the host over VirtioFS, needs two settings:
- `CARGO_TARGET_DIR` on a local disk, because rustc can't memory-map its fresh objects there.
- `-Pearshot.runDir=<local dir>` for the game runs, because Minecraft can't lock its saves there.

In game, `/earshot rays [count] [bounces]` draws traced sound paths, `/earshot occlusion` measures what blocks the way ahead, `/earshot sources` shows how each playing sound is heard, and `/earshot stats` shows how much of the world is mirrored.

Release jars hold the native library for every platform. CI builds each one on its own runner and packages them with `cargo xtask dist --from <dir>`. Pushing a `v<version>` tag publishes a prerelease.

## Running the mod

Earshot loads its native library through FFM. Like Yak VC, it needs this in your launcher's JVM arguments:

```
--enable-native-access=ALL-UNNAMED
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. The build setup and native loader are adapted from Yak VC, under the same licenses.
