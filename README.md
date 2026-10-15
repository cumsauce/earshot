# Earshot

Physically based acoustics for Minecraft 26.3: occlusion, sound bending around corners, raytraced reverb and echo, and binaural (HRTF) audio. It applies to the game's own sounds and, through [Yak VC](https://github.com/desocketed/yakvc)'s voice sink ([desocketed/yakvc#174](https://github.com/desocketed/yakvc/issues/174)), to proximity voice. It is a client-only Fabric mod.

Status: **alpha (0.1.0-alpha.39)**, on the [releases page](https://github.com/cumsauce/earshot/releases). Game sounds are muffled by what is in the way, come round corners from the opening they pass through, fade with air absorption, shift with Doppler, and are muffled underwater. Each also feeds a reverb measured from the space around you: a convolution with an impulse response synthesized from the room where OpenAL Soft has the effect, EAX reverb otherwise. Water has a surface: crossing it costs the impedance jump once per crossing, and under it sound carries across an ocean. Slabs, stairs and wall posts pass sound by their thickness (the mass law), thin walls leak it, and carpets and snow layers soak up the highs. A mineshaft corridor rings; a leaf-walled treehouse is nearly outdoors. Yak VC voices, with its voice sink, get the same treatment, including your own in a cave. Settings are under Options → Music & Sounds → Acoustics….

**Players:** see the [user guide](docs/GUIDE.md). [docs/PLAN.md](docs/PLAN.md) has the design, the milestones and the measurements so far.

## How it works

- **Simulation (Rust).** A Rust engine, loaded through Java's Foreign Function & Memory API, keeps a voxel copy of the blocks around you. It traces rays through them to work out occlusion, the paths sound takes around walls, and the reverb of the space you're in. Each mirrored block carries its material's id and how much of the block is there, so a slab passes half a slab's worth of sound.
- **Rendering (OpenAL Soft).** The OpenAL Soft that ships with Minecraft renders the result:
  - EFX filters for muffling;
  - EAX or convolution reverb, with impulse responses synthesized from the raytrace;
  - HRTF for direction.

## Testing

The suite runs the simulation against closed forms rather than its own constants, so a bug in a table or a threshold shows up as a failing test, not a slow drift in how the game sounds:

- **Closed forms.** Room decay against Eyring's formula (and the leaky-wall form beside it) for seven materials, mean free path against 4V/S, the water surface against the impedance-ratio figure, echoes against their image sources, the rug against published carpet coefficients, wall loss against the mass law.
- **Determinism.** Fixed seeds, integer RNG, IEEE arithmetic: the same trace gives the same answer, on every machine, checked through the threaded path too.
- **Property tests.** The DDA against a brute-force block walk, the flood fill against plain Dijkstra, transmission against a slow walk, `hear`'s gains bounded over thousands of random worlds and sources.
- **Golden scenes** (`crates/earshot-sim/tests/scenes.rs`): a sealed stone hut, the same with a 1 × 2 doorway, open ground, a carpeted hall, a ravine and an ocean, each with recorded expectations — enclosure, decay, the way round, and how a source outside is heard. These pins are how the missing wall-leak channel was found.
- **Client gametest.** CI builds a world in a real client and checks that a sound in a closed stone box is muffled, that its doorway routes it, that a stone hall's reverb responds, and that the block-to-material table says what it should.
- **Benches.** The path tracer (`--example trace_bench`) and OpenAL's render cost (`./gradlew alBench`) run on CI's runners; their numbers live in [docs/PLAN.md](docs/PLAN.md).

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

In game, `/earshot rays [count] [bounces]` draws traced sound paths, `/earshot sources` shows how each playing sound is heard, and `/earshot room` describes the room the reverb hears. [The guide](docs/GUIDE.md#commands) lists every command.

Release jars hold the native library for every platform. CI builds each one on its own runner and packages them with `cargo xtask dist --from <dir>`. Pushing a `v<version>` tag publishes a prerelease.

## Running the mod

Earshot loads its native library through FFM. Like Yak VC, it needs this in your launcher's JVM arguments:

```
--enable-native-access=ALL-UNNAMED
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. The build setup and native loader are adapted from Yak VC, under the same licenses.

The KU100 and KEMAR HRTFs come from the [SADIE II database](https://www.york.ac.uk/sadie-project/database.html), copyright University of York 2018, under the Apache License 2.0 (C. Armstrong, L. Thresh, D. Murphy and G. Kearney, "A Perceptual Evaluation of Individual and Non-Individual HRTFs: A Case Study of the SADIE II Database", *Applied Sciences* 8(11), 2029, 2018, [doi:10.3390/app8112029](https://doi.org/10.3390/app8112029)). They were converted to OpenAL Soft's format with its `makemhr`; [the notice](mod/src/main/resources/earshot/hrtf/NOTICE.txt) says how.
