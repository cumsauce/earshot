# Earshot

Physically based acoustics for Minecraft 26.3: occlusion, sound bending around corners, raytraced reverb and echo, and binaural (HRTF) audio. It applies to the game's own sounds and, through [Yak VC](https://github.com/desocketed/yakvc)'s voice sink ([desocketed/yakvc#174](https://github.com/desocketed/yakvc/issues/174)), to proximity voice. It is a client-only Fabric mod.

Status: **M0**, groundwork. Nothing is audible yet. [docs/PLAN.md](docs/PLAN.md) has the design, the milestones and the measurements so far.

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

Release jars hold the native library for every platform. CI builds each one on its own runner and packages them with `cargo xtask dist --from <dir>`. Pushing a `v<version>` tag publishes a prerelease.

## Running the mod

Earshot loads its native library through FFM. Like Yak VC, it needs this in your launcher's JVM arguments:

```
--enable-native-access=ALL-UNNAMED
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. The build setup and native loader are adapted from Yak VC, under the same licenses.
