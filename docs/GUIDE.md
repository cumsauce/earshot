# Earshot user guide

Earshot makes Minecraft's own sounds behave like sound in a real space. It changes nothing about what the game plays,
only how you hear it:

- **Occlusion.** Blocks between you and a sound muffle it by what they are made of. Wool deadens, glass lets a lot
  through, stone blocks almost everything. A closed wooden door muffles; an open one doesn't.
- **Round corners.** A sound behind a wall comes round through the nearest opening, and you hear it from that
  direction, a little duller for each bend.
- **Reverb and echo**, measured from the space around you by tracing rays: a stone hall rings for seconds, a small
  wooden room barely at all, an open field not at all. Where the game's OpenAL can do it, the reverb is a convolution
  with an impulse response made from the room, with early echoes from where the walls actually are. On High and Ultra,
  a sound in another space (a cave you stand outside, the next room) rings with that space's reverb, coming from its
  direction.
- **Air absorption, underwater muffling and Doppler.**
- **Thunder arrives late**, as sound takes to come from the bolt: about three seconds a kilometre after the flash.
- **The Nether rings like the cavern it is**: under a dimension's ceiling, Earshot traces rays twice as far.
- **Proximity voice chat**, if you use [Yak VC](https://github.com/desocketed/yakvc): voices get all of the above too.

It is a client-only Fabric mod. Servers need nothing.

## Installing

You need Minecraft 26.3, Fabric Loader 0.19.5 or later, Fabric API, and Java 25 (the game's own runtime is fine).

1. Put the Earshot jar from the [releases page](https://github.com/cumsauce/earshot/releases) in your `mods` folder,
   with Fabric API.
2. Add this to your launcher's JVM arguments. Earshot's engine is native code loaded through Java's Foreign Function
   & Memory API, which needs it (Yak VC needs the same):

   ```
   --enable-native-access=ALL-UNNAMED
   ```

The jar carries the engine for Windows x86-64, macOS (Apple Silicon and Intel) and Linux x86-64 and ARM64. On other
platforms Earshot logs that it is off and the game plays as usual.

**Turn on Directional Audio** (Options → Music & Sounds) and use headphones. That is the game's switch for HRTF, which
is what lets you hear whether a sound is above, behind or in front of you.

Earshot can't run with Sound Physics Remastered, which does the same job; Fabric refuses to start with both.

## Settings

Options → Music & Sounds → **Acoustics…**, or Earshot's config button in Mod Menu. The screen has sections for
Sound, Reverb, Headphones and Voice Chat; settings that can't take effect (any effect while Quality is Off, HRTF while
Directional Audio is off, Own Voice Echo while Voice Chat is off) are greyed out. Under Quality it shows what Auto
picked and what tracing costs right now; under Reverb, which reverb is playing. Changes apply when you leave the screen and are saved in
`config/earshot/earshot.json`.

| Setting | What it does |
| --- | --- |
| Quality | How much Earshot traces (below). Off leaves the game's sounds as they are. |
| Occlusion | Blocks in the way muffle sounds. |
| Round Corners | Sounds come round through openings. |
| Reverb | Auto (convolution where the game's OpenAL has it, else EAX), Convolution, EAX, or Off. |
| Air Absorption | Far sounds lose their highs. |
| Underwater | Everything is muffled while your head is underwater. |
| Realistic Falloff | Off by default. Sounds fade as in real air, halving with each doubling of distance past two blocks, instead of the game's straight line to silence. |
| Doppler | Sounds moving towards or away from you shift in pitch. |
| Voice Chat | Plays Yak VC's voice through Earshot. Off gives it back to Yak VC's own speakers. |
| Own Voice Echo | Off by default. Your own voice in Yak VC echoes off the room around you (only the echo, never your voice itself). For headphones, and needs a Yak VC that sends your own voice; with an older one Earshot notices and leaves it off. |
| Directional Audio | The game's own switch for HRTF, shown here too. |
| HRTF | Which head-related transfer function Directional Audio uses: OpenAL Soft's Built-In one (the default), or one of two dummy heads from the SADIE II database, the Neumann KU100 and the KEMAR. Try each with headphones; the one that places sounds best for you suits your ears. Changing it restarts the sound engine. |

To hear the difference, bind **Bypass (A/B)** under Options → Controls → Key Binds → Earshot. Each press switches
Earshot off and on again; your settings stay as they are.

### Quality

| Quality | Room trace | Occlusion rays per sound | Round corners within |
| --- | --- | --- | --- |
| Low | 256 rays × 4 bounces, 4 times a second | 1 | 16 blocks |
| Medium | 1,024 × 8, 8 times a second | 5 | 24 blocks |
| High | 4,096 × 16, 10 times a second | 9 | 32 blocks |
| Ultra | 16,384 × 32, 10 times a second | 9 | 48 blocks, and the four loudest sounds get early echoes off the walls around them |

**Auto** picks High on 8 or more cores, Medium on 4 to 7, and Low below that. At any quality the room trace has a
share of the CPU (from a tenth of a core on Low to six tenths on Ultra), and uses fewer rays when a big space would take
more. The room trace splits over up to four threads, so on Ultra it can take over a core in all without slowing any
one. On an M3 MacBook, High costs about a quarter of a core, plus about a tenth for the convolution reverb.

## Voice chat

With [Yak VC](https://github.com/desocketed/yakvc) installed, Earshot offers to play its proximity voice, and Yak VC
hands the voices over on its own. Each speaker then sounds from where they stand, with the same occlusion, corners,
reverb and HRTF as the game's sounds: a friend shouting from inside a cave sounds like it. Group voice and calls stay
centred and dry, as in Yak VC. The game's Voice/Speech volume applies.

This needs a Yak VC with the voice sink, which is in Yak VC's `main` since 2026-10-12 (desocketed/yakvc#177, and
#186 for Own Voice Echo) and will be in its next release. Older versions keep playing voice through their own
speakers, as if Earshot weren't there.

`/earshot voice` shows who is playing through Earshot.

## Materials

Earshot decides what each block is made of from its sound, its tags and its shape, so modded blocks get something
sensible without a list. The materials are air, rock, wood, glass, cloth, leaves, soil, sand, metal, water, lava, ice
and snow.

To check a block, look at it and run `/earshot material`. To change one, write `config/earshot/materials.json`,
mapping block ids or block tags (with `#`) to materials, then run `/earshot materials reload`:

```json
{
	"minecraft:hay_block": "cloth",
	"#minecraft:logs": "wood",
	"somemod:acoustic_foam": "cloth",
	"somemod:mesh_fence": "air"
}
```

Block ids win over tags. Blocks that let sound through (open doors, fences, carpets and other thin shapes) stay open
unless you set them to something else.

## Commands

| Command | What it shows or does |
| --- | --- |
| `/earshot sources` | How each playing sound is heard: muffling per band, and the opening it comes round through. |
| `/earshot room` | The room the reverb hears: decay times, size, how enclosed. |
| `/earshot reverb auto\|convolution\|eax\|off` | Switches the reverb. |
| `/earshot rays [count] [bounces]` | Draws traced sound paths from where you stand. |
| `/earshot occlusion` | What blocks the way ahead of you. |
| `/earshot voice [on\|off]` | The voices playing through Earshot, and the Voice Chat setting. |
| `/earshot material` | The material of the block you look at. |
| `/earshot materials reload` | Reads `materials.json` again. |
| `/earshot stats` | How much of the world is copied, and what the trace is costing. |
| `/earshot overlay` | A debug overlay in the top-left corner: quality and cost, the room, the reverbs playing, and the nearest sounds. It can also go on a key: **Debug Overlay** under Controls → Earshot. |

## Troubleshooting

- **No change to any sound, and the log says "Earshot is off: could not load the native engine".** The JVM argument
  is missing, or your platform has no engine in the jar.
- **The log says the OpenAL probe found no EFX.** Something replaced the game's OpenAL Soft. Earshot needs its EFX
  extension for every effect.
- **Stutters when a big cave opens up.** Pick a lower Quality. Earshot's tracing runs on its own thread, but a weak CPU
  shares its cores with the game.
- **Reverb sounds too strong or too weak.** Try Reverb: EAX against Convolution. Reports with `/earshot room` output
  help: [issues](https://github.com/cumsauce/earshot/issues).

## Not yet

Earshot is an alpha.

Earshot puts the two SADIE II HRTFs (`SADIE II KU100.mhr`, `SADIE II KEMAR.mhr`) in the top of your game folder at
startup, because that is where the game's OpenAL looks for them. They're about 750 KB each. Deleting them is harmless:
they come back next time, and the built-in HRTF is always there.
