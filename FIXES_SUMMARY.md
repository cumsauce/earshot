# Summary of Fixes for Earshot Sim Core

## Fixed Bugs

### Bug 1: HIGH - Air absorption missing from EDC curve (reverb.rs)
- **Location**: `crates/earshot-sim/src/reverb.rs`, lines 302-338 → 302-347
- **Problem**: `Tally::add` reduced `band_energy` only by surface absorption/transmission, but the EDC histogram was built from this airless energy. Meanwhile `Tally::room` computed decay with explicit air term, causing `early_decay` to systematically exceed `decay` by the air share of loss.
- **Fix**: Added air absorption decay to `band_energy` before depositing into histogram:
  ```rust
  // The air takes its own share of the energy as the ray travels this
  // leg. The curve's slope must fall by that share too: `Tally::room`
  // adds it as `AIR · mean_free_path` to the Eyring loss, so a curve
  // that kept the air's energy would read an early decay longer than
  // the room's own by the air's share of the loss. Not added to
  // `absorbed` — that term is the air's, counted once there.
  for (energy, air) in self.band_energy.iter_mut().zip(AIR) {
      *energy *= (-air * length).exp();
  }
  ```

### Bug 2: MEDIUM - Three different transmission models for partial blocks
- **Location**: 
  - `crates/earshot-sim/src/paths.rs` (escape probability)
  - `crates/earshot-sim/src/reverb.rs` (tally loss and early reflection leak)
- **Problem**: Three different transmission models:
  - `paths.rs`: linear `T * fill`
  - `reverb.rs` (tally loss): exponential `T^fill`
  - `reverb.rs` (early reflection leak): linear `leak = 1 - T*fill`
  - World crate (authoritative): exponential `through^(length*fill)`
- **Fix**: Unified on `transmission^(traversed_thickness * fill)` everywhere:
  - Added `thickness: f32` to `Segment` struct (distance ray travels through block)
  - Updated `paths.rs`: escape probability uses `transmission.powf(thickness * fill)`
  - Updated `reverb.rs`: tally loss and leak use `transmission.powf(thickness * fill)`
  - `traversed_thickness = hit.exit - hit.distance`

### Bug 3: LOW - Direction NaN when energy == 0 (early.rs)
- **Location**: `crates/earshot-sim/src/early.rs`, lines 73-81
- **Problem**: `direction: sum * (1.0 / f64::from(energy))` is NaN when `energy == 0`. Unreachable today but reachable if a material with `absorption==1` or `scattering==1` is added.
- **Fix**: Added guard before normalizing:
  ```rust
  let energy: f32 = taps.iter().map(|t| t.energy).sum();
  // A tap whose energy is all gone (a material that absorbs or scatters
  // everything) leaves nothing to weight a direction by: normalising by
  // that zero would give a NaN direction.
  if energy <= 0.0 {
      return Early::NONE;
  }
  ```

### Bug 4: LOW - fit_early_decay uses hist[0] as peak, not curve max (reverb.rs)
- **Location**: `crates/earshot-sim/src/reverb.rs`, line 483
- **Problem**: Assumes EDC peaks in bin 0. For listener against wall, first leg can be under 1 ms bin, so bin 1+ may be higher, biasing fitted slope.
- **Fix**: Use actual curve maximum as peak:
  ```rust
  // The curve's own peak, not bin 0's: from a listener against a wall the
  // first legs can be shorter than a bin, so the energy keeps arriving for
  // a bin or two and the curve peaks past bin 0. Reading bin 0 as the peak
  // there makes the fit start on a rising curve and biases its slope.
  let peak = hist.iter().copied().fold(0.0, f64::max);
  if peak <= 0.0 || !peak.is_finite() {
      return fallback;
  }
  ```

### Bug 5: LOW - .max(f64::MIN_POSITIVE) silently converts NaN EDC bin (reverb.rs)
- **Location**: `crates/earshot-sim/src/reverb.rs`, line 487
- **Problem**: `f64::max(NaN, x)` returns `x`, so NaN `hist[bin]` becomes -3076 dB, forces fallback.
- **Fix**: Check for finite values before processing:
  ```rust
  let level = |bin: usize| {
      let value = hist[bin];
      // A bin that isn't a number has no level: treat it as silence rather
      // than let `f64::max`'s NaN-swallowing turn it into −3076 dB.
      if !value.is_finite() {
          return f64::NEG_INFINITY;
      }
      10.0 * (value / peak).max(f64::MIN_POSITIVE).log10()
  };
  ```

## Test Results
All 212 tests pass:
- 30 FFI tests
- 135 sim tests  
- 1 BRAS test
- 1 scenes test
- 35 world tests
- 10 other tests

## Files Modified
- `crates/earshot-sim/src/early.rs`
- `crates/earshot-sim/src/paths.rs` 
- `crates/earshot-sim/src/reverb.rs`
- `crates/earshot-sim/tests/reverb_probe.rs` (updated test assertion for air absorption)