**Update after #170–#172 and the `sound-paths` branch**

Since I opened this, main has gained occlusion, material occlusion and room reverb for voice (ABI 9), and `sound-paths` adds paths around walls. Nice work. Two adjustments to the request, plus one optional one:

1. **ABI numbers.** Read "7 → 8" as "the next ABI version": 11, now that `sound-paths` (#173) has landed as ABI 10.
2. **The sink gets the frame before any of yakvc's acoustics.** While a sink is set, also skip:
   - the occlusion gain cut in `occluded()` and the per-remote `Muffle` (`remote.muffle.apply` in `AudioLoop::play`);
   - the reverb send and the `Mixer`'s `Reverb` (including `set_reverb_decay`);
   - `heard_from` from `SoundPath`. The renderer positions voices itself.

   The renderer applies its own occlusion, paths and reverb to game sounds and voice together, so yakvc applying them as well would double them. Range and recipients still use the real position, as they do now.
3. **Optional, saves CPU.** While a sink is set, `GameStateFeeder` could skip `Occlusion`, `SoundPath` and `Room` and push zeros, since nothing would use them.

Everything else stands as written: the rules (mute, deafen, friends_only, groups, spectators, blocked, range) still decide what reaches the sink, and nothing changes without one.
