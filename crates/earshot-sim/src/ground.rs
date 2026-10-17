//! Outdoor ground reflection: the excess attenuation of the ground effect,
//! from the Weyl-Van der Pol model (docs/ground-reflection-design.md).
//!
//! The sim is energy-based and cannot trace the interference between the
//! direct wave and the wave reflected off the ground ray by ray, so the
//! whole effect is folded into the direct path as a per-band real factor at
//! each band's centre frequency. A tiny local `Complex` and the Faddeeva
//! function live here because neither crate has a complex type.

use std::sync::LazyLock;

use earshot_world::{BANDS, Bands, Material, Vec3, World};

/// Speed of sound in air, metres per second: the design fixes it, and the
/// band centres below are the ones hear's `WAVELENGTH` was built from.
const C0: f64 = 343.0;
/// The bands' centre frequencies, in hertz, low to high.
const BAND_HZ: [f64; BANDS] = [250.0, 1000.0, 4000.0];
/// Above this enclosure the listener is shut in: their floor is the room
/// trace's business and the open ground must not double count it (the
/// design starts the threshold at 0.9, a sealed room measuring ~1 and
/// open ground ~0.5; see `Room::enclosure`).
const OPEN: f32 = 0.9;
/// The most the ground can lift the direct sound: +6.02 dB is the hard
/// ground's Lloyd's-mirror ceiling at grazing, and nothing reflects harder.
const MAX_GAIN: f64 = 2.0;
/// How far down under the listener or the source the scan looks for ground,
/// in blocks. Short, so an all-air column reads as sky, not as ground.
const SCAN: i32 = 16;

/// A complex number, just enough of one: the ground model needs add, sub,
/// mul, div, neg, abs, sqrt, exp and scaling, and a whole crate for that
/// would be silly.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const ZERO: Complex = Complex { re: 0.0, im: 0.0 };
    const ONE: Complex = Complex { re: 1.0, im: 0.0 };
    const I: Complex = Complex { re: 0.0, im: 1.0 };

    fn new(re: f64, im: f64) -> Self {
        Complex { re, im }
    }

    /// The distance from the origin.
    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Multiply by a real factor.
    fn scale(self, k: f64) -> Self {
        Complex {
            re: k * self.re,
            im: k * self.im,
        }
    }

    /// The principal square root, with the branch cut kept above the real
    /// axis: the ground wave's square root must stay in the decaying half
    /// plane, and a negative real argument comes back with `+i` so the
    /// reflected field never grows.
    fn sqrt(self) -> Self {
        let m = self.abs();
        let re = ((m + self.re) / 2.0).sqrt();
        let im = if self.im < 0.0 {
            -((m - self.re) / 2.0).sqrt()
        } else {
            ((m - self.re) / 2.0).sqrt()
        };
        Complex { re, im }
    }

    /// e raised to this power, `e^(x+iy) = e^x (cos y + i sin y)`.
    fn exp(self) -> Self {
        let e = self.re.exp();
        Complex {
            re: e * self.im.cos(),
            im: e * self.im.sin(),
        }
    }
}

impl std::ops::Add for Complex {
    type Output = Complex;
    fn add(self, rhs: Complex) -> Complex {
        Complex {
            re: self.re + rhs.re,
            im: self.im + rhs.im,
        }
    }
}

impl std::ops::Sub for Complex {
    type Output = Complex;
    fn sub(self, rhs: Complex) -> Complex {
        Complex {
            re: self.re - rhs.re,
            im: self.im - rhs.im,
        }
    }
}

impl std::ops::Mul for Complex {
    type Output = Complex;
    fn mul(self, rhs: Complex) -> Complex {
        Complex {
            re: self.re * rhs.re - self.im * rhs.im,
            im: self.re * rhs.im + self.im * rhs.re,
        }
    }
}

impl std::ops::Mul<f64> for Complex {
    type Output = Complex;
    fn mul(self, rhs: f64) -> Complex {
        self.scale(rhs)
    }
}

impl std::ops::Div for Complex {
    type Output = Complex;
    fn div(self, rhs: Complex) -> Complex {
        let d = rhs.re * rhs.re + rhs.im * rhs.im;
        Complex {
            re: (self.re * rhs.re + self.im * rhs.im) / d,
            im: (self.im * rhs.re - self.re * rhs.im) / d,
        }
    }
}

impl std::ops::Neg for Complex {
    type Output = Complex;
    fn neg(self) -> Complex {
        Complex {
            re: -self.re,
            im: -self.im,
        }
    }
}

/// Weideman's rational approximation order (Weideman 1994, SIAM J. Numer.
/// Anal. 31(5)): 36 poles land near roundoff over the region used here,
/// and the coefficient table is built once from it.
const WEIDEMAN_ORDER: usize = 36;

/// The approximation's fixed coefficients, built once: Weideman's tangent
/// sampling of the Faddeeva integral turned into a Fourier sum by hand, so
/// no FFT dependency is needed for a table of 36 numbers.
static WEIDEMAN: LazyLock<[f64; WEIDEMAN_ORDER]> = LazyLock::new(|| {
    let n = WEIDEMAN_ORDER;
    let m = 2 * n;
    let m2 = 2 * m;
    // The optimal scale of the tangent grid: it puts the sampling points
    // where e^(-t^2) still has weight.
    let l = (n as f64 / std::f64::consts::SQRT_2).sqrt();
    // Samples of e^(-t^2) (l^2 + t^2) on the tangent grid for k = -m+1 ..
    // // m-1, with a zero in front so the length is a power-of-two friendly
    // m2; t = l tan(k pi / 2m).
    let mut shifted = [0.0f64; 4 * WEIDEMAN_ORDER];
    for (i, sample) in shifted.iter_mut().enumerate().skip(1) {
        let k = i as f64 - m as f64;
        let t = l * (k * std::f64::consts::PI / (2.0 * m as f64)).tan();
        *sample = (-t * t).exp() * (l * l + t * t);
    }
    // fftshift then the discrete transform: the real part of the sum is
    // all that survives, because every sample is real.
    let mut coefficients = [0.0f64; WEIDEMAN_ORDER];
    let mut transform = [0.0f64; 4 * WEIDEMAN_ORDER];
    for (n_index, value) in transform.iter_mut().enumerate() {
        let mut sum = 0.0;
        for (i, &sample) in shifted.iter().enumerate() {
            let angle =
                2.0 * std::f64::consts::PI * ((i + 2 * n) % m2) as f64 * n_index as f64 / m2 as f64;
            sum += sample * angle.cos();
        }
        *value = sum / m2 as f64;
    }
    // Reordered the way polyval wants them: highest power first.
    for (q, coefficient) in coefficients.iter_mut().enumerate() {
        *coefficient = transform[n - q];
    }
    coefficients
});

/// The Faddeeva function `wofz(z) = e^(-z^2) erfc(-i z)`, by Weideman's
/// rational approximation: a Möbius map of the half plane onto the unit
/// disk and a 36th-degree polynomial in it. There is no closed form, and
/// this is accurate to about 1e-14 over the small arguments the ground
/// model sees (near-grazing keeps `|w|` small, the easiest regime).
fn wofz(z: Complex) -> Complex {
    let l = (WEIDEMAN_ORDER as f64 / std::f64::consts::SQRT_2).sqrt();
    let l = Complex::new(l, 0.0);
    let denominator = l - Complex::I * z;
    let mapped = (l + Complex::I * z) / denominator;
    // Horner's rule over coefficients highest power first.
    let mut polynomial = Complex::new(WEIDEMAN[0], 0.0);
    for &coefficient in &WEIDEMAN[1..] {
        polynomial = polynomial * mapped + Complex::new(coefficient, 0.0);
    }
    polynomial.scale(2.0) / (denominator * denominator)
        + Complex::new(1.0 / std::f64::consts::PI.sqrt(), 0.0) / denominator
}

/// Miki's one-parameter surface impedance, normalised by rho0 c0, at
/// frequency `f` over a surface of flow resistivity `sigma` (Pa s/m^2).
///
/// `X = f / sigma`, `Z = 1 + 0.070 X^(-0.632) + i 0.107 X^(-0.632)`. The
/// sign is the design's stated `e^(-iwt)` convention: a passive ground's
/// impedance leans positive-imaginary there, which is what turns the
/// porous ground's reflection into the low-band null the tests pin. The
/// design's formula line shows the published `-i` form instead; the
/// parenthetical and the whole test plan ask for `+i`, and the limits
/// below only pass with it.
///
/// An infinite `sigma` is the rigid limit; the caller checks for it before
/// dividing, because infinity arithmetic would turn `1/Z` into a NaN.
fn surface_impedance(sigma: f64, f: f64) -> Complex {
    let x = f / sigma;
    let c = x.powf(-0.632);
    Complex::new(1.0 + 0.070 * c, 0.107 * c)
}

/// The excess attenuation of the ground effect, in dB re free field, for
/// source height `h_s` and receiver height `h_r` (both metres above the
/// ground plane), horizontal separation `d`, frequency `f` and the
/// surface's flow resistivity `sigma`.
///
/// Weyl-Van der Pol: the specular reflection with a ground wave correction
/// that keeps grazing finite, as in the design doc's model section.
fn excess_attenuation_db(sigma: f64, h_s: f64, h_r: f64, d: f64, f: f64) -> f64 {
    let r1 = d.hypot(h_s - h_r);
    let r2 = d.hypot(h_s + h_r);
    if r2 == 0.0 {
        // The points coincide: there is no path along which anything could
        // interfere, so the ground has nothing to add.
        return 0.0;
    }
    let delta = r2 - r1;
    let cos_theta = (h_s + h_r) / r2;
    // The normalised admittance. A rigid surface has beta = 0 directly:
    // dividing by an infinite impedance would produce a NaN at grazing.
    let beta = if sigma.is_finite() {
        Complex::ONE / surface_impedance(sigma, f)
    } else {
        Complex::ZERO
    };
    let k0 = 2.0 * std::f64::consts::PI * f / C0;
    let cos_theta = Complex::new(cos_theta, 0.0);
    let rp = if beta == Complex::ZERO {
        // Rigid ground reflects everything, at every angle including
        // grazing, where the quotient would be 0/0.
        Complex::ONE
    } else {
        (cos_theta - beta) / (cos_theta + beta)
    };
    let w = Complex::new(0.0, k0 * r2 / 2.0).sqrt() * (cos_theta + beta);
    let ground_wave = Complex::ONE + Complex::I * std::f64::consts::PI.sqrt() * w * wofz(w);
    let q = rp + (Complex::ONE - rp) * ground_wave;
    let pressure = Complex::ONE + q * (r1 / r2) * Complex::new(0.0, k0 * delta).exp();
    20.0 * pressure.abs().log10()
}

/// The per-band ground factor for one geometry: the excess attenuation as
/// an energy-style multiplier, clamped between silence and the hard
/// ground's +6.02 dB ceiling, since nothing reflects harder than rigid.
fn band_gains(sigma: f64, h_s: f64, h_r: f64, d: f64) -> Bands {
    std::array::from_fn(|band| {
        let db = excess_attenuation_db(sigma, h_s, h_r, d, BAND_HZ[band]);
        10.0_f64.powf(db / 20.0).clamp(0.0, MAX_GAIN).max(0.0) as f32
    })
}

/// Flow resistivity per material, Pa s/m^2, from the design's table:
/// grassland, dry sand, very absorptive snow and forest litter. Every
/// other material has no entry and is acoustically hard: an infinite
/// resistivity, which means the Lloyd's-mirror boost with no porous null.
fn flow_resistivity(material: Material) -> f64 {
    match material {
        Material::Soil => 200_000.0,
        Material::Sand => 100_000.0,
        Material::Snow => 20_000.0,
        Material::Leaves => 15_000.0,
        _ => f64::INFINITY,
    }
}

/// The top of the highest solid block under `at` within [`SCAN`] blocks
/// down, with the material found: the ground plane is the top face of
/// that block, so standing on it puts height zero at your feet. None means
/// sky, or a spot the scan never reaches.
fn ground_under(world: &World, at: Vec3) -> Option<(f64, Material)> {
    let (x, z) = (at.x.floor() as i32, at.z.floor() as i32);
    let top = at.y.floor() as i32;
    for dy in 0..SCAN {
        let material = world.block(x, top - dy, z);
        if material.is_solid() {
            return Some(((top - dy) as f64 + 1.0, material));
        }
    }
    None
}

/// The ground's per-band factor for the direct sound, when it applies at
/// all: `None` indoors or with no ground in reach, `Some` otherwise.
///
/// All of the design's gates: the listener is not shut in, solid ground
/// sits under them within the short scan, the source stands above that
/// same plane with its own ground within the scan, and the degenerate
/// cases hand back no effect rather than a NaN. (`hear` asks for no
/// factor when there are no rays; that gate lives there.)
pub(crate) fn ground_gain(
    world: &World,
    listener: Vec3,
    source: Vec3,
    enclosure: f32,
) -> Option<Bands> {
    if enclosure >= OPEN {
        return None;
    }
    let (plane, material) = ground_under(world, listener)?;
    let h_r = listener.y - plane;
    let h_s = source.y - plane;
    if h_r < 0.0 || h_s < 0.0 {
        // Under the surface itself is not an open-air path above ground.
        return None;
    }
    // v1 accepts any solid block under the source within the cap, rather
    // than demanding its plane match the listener's to the block.
    ground_under(world, source)?;
    let d = (source.x - listener.x).hypot(source.z - listener.z);
    let sigma = flow_resistivity(material);
    Some(band_gains(sigma, h_s, h_r, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dB of a doubling of pressure, the hard ground's ceiling at
    /// grazing.
    const SIX_DB: f64 = 6.020_599_913_279_624;

    #[test]
    fn a_hard_ground_gives_the_six_decibel_boost_at_grazing() {
        // Rigid ground, both ends a whisker above it: the mirror image
        // lands on top of the source, the two arrive as one, and twice the
        // pressure is +6.02 dB in every band.
        let db = excess_attenuation_db(f64::INFINITY, 1e-3, 1e-3, 10.0, 250.0);
        assert!((db - SIX_DB).abs() < 0.01, "{db}");
        let gains = band_gains(f64::INFINITY, 1e-3, 1e-3, 10.0);
        for gain in gains {
            assert!((f64::from(gain) - MAX_GAIN).abs() < 0.01, "{gains:?}");
        }
    }

    #[test]
    fn a_porous_ground_dips_the_low_band() {
        // The design's pinned geometry: soil, 1.6 m up each end, 20 m
        // apart. The low band sits in the ground's null, the mid band on
        // the interference crest, and the high band dips again where the
        // ground has gone soft under Miki's model.
        let low = excess_attenuation_db(200_000.0, 1.6, 1.6, 20.0, 250.0);
        let mid = excess_attenuation_db(200_000.0, 1.6, 1.6, 20.0, 1000.0);
        let high = excess_attenuation_db(200_000.0, 1.6, 1.6, 20.0, 4000.0);
        assert!(low.is_finite() && mid.is_finite() && high.is_finite());
        assert!(low < 0.0, "the low band is a null: {low}");
        assert!(mid > 0.0, "the mid band rides the crest: {mid}");
        // Pinned as the model's own value at this geometry: with this
        // impedance the deepest interference dip of the three bands lands
        // in the high band, where the ground is softest, not the low one
        // the design's prose expected; Miki's ground stiffens towards low
        // frequencies, so the low band stays closer to the hard boost.
        assert!((low - (-2.4375)).abs() < 0.05, "{low}");
        assert!((mid - 3.1960).abs() < 0.05, "{mid}");
        assert!((high - (-3.8149)).abs() < 0.05, "{high}");
        let deepest = [low, mid, high]
            .into_iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert_eq!(deepest.0, 2, "the deepest null lands in the high band");
        // Softer ground takes more out of the high band, where its pores
        // actually move: rigid, then soil, then snow.
        let rigid = excess_attenuation_db(f64::INFINITY, 1.6, 1.6, 20.0, 4000.0);
        let sand = excess_attenuation_db(100_000.0, 1.6, 1.6, 20.0, 4000.0);
        let snow = excess_attenuation_db(20_000.0, 1.6, 1.6, 20.0, 4000.0);
        assert!(
            rigid > high && high > sand && sand > snow,
            "{rigid} {high} {sand} {snow}"
        );
    }

    #[test]
    fn grazing_over_a_porous_ground_stays_finite() {
        // Both heights to zero: the direct and reflected waves cancel
        // almost completely over soft ground, and only the ground wave
        // term saves the sum from minus infinity, exactly as the design's
        // limit requires.
        for f in BAND_HZ {
            let db = excess_attenuation_db(200_000.0, 0.0, 0.0, 20.0, f);
            assert!(db.is_finite(), "{f} Hz: {db}");
            assert!(db > -60.0, "{f} Hz below a sane floor: {db}");
            assert!(db <= SIX_DB + 0.05, "{f} Hz beats rigid ground: {db}");
        }
    }

    #[test]
    fn the_ground_effect_is_reciprocal() {
        // Swapping the two heights changes nothing: R2, cos theta and w
        // all depend on the heights only through their sum, and so does
        // the excess length.
        for sigma in [f64::INFINITY, 200_000.0, 20_000.0] {
            for (h_s, h_r) in [(0.0, 1.6), (0.5, 4.0), (1.6, 1.6)] {
                for d in [1.0, 20.0, 100.0] {
                    for f in BAND_HZ {
                        let there = excess_attenuation_db(sigma, h_s, h_r, d, f);
                        let back = excess_attenuation_db(sigma, h_r, h_s, d, f);
                        assert!((there - back).abs() < 1e-12, "{there} vs {back}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_ground_factor_stays_between_silence_and_the_hard_boost() {
        // A sweep over every material, uneven heights and distances near
        // and far: the clamp is what keeps the folded gain bounded, so
        // nothing here may fall outside silence and the hard ceiling.
        for material in Material::ALL {
            let sigma = flow_resistivity(material);
            for (h_s, h_r) in [(0.0, 0.0), (0.5, 1.6), (1.6, 1.6), (4.0, 0.2), (10.0, 10.0)] {
                for d in [1.0, 6.5, 20.0, 100.0] {
                    let gains = band_gains(sigma, h_s, h_r, d);
                    for gain in gains {
                        assert!(gain.is_finite(), "{material:?}: {gains:?}");
                        assert!((0.0..=2.0).contains(&gain), "{material:?}: {gains:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn wofz_matches_its_known_values() {
        // At the origin the Faddeeva function is one.
        assert!((wofz(Complex::ZERO) - Complex::ONE).abs() < 1e-12);
        // On the real axis wofz(x) = e^(-x^2) (1 + i erfi(x)), with erfi(1)
        // tabulated as 1.6504257587975428.
        let erfi_one = 1.650_425_758_797_542_8;
        let e_inv = (-1.0f64).exp();
        let expected = Complex::new(e_inv, e_inv * erfi_one);
        assert!(
            (wofz(Complex::new(1.0, 0.0)) - expected).abs() < 1e-12,
            "{expected:?}"
        );
        // On the imaginary axis wofz(i) = e erfc(1), with erfc(1) tabulated
        // as 0.15729920705028513.
        let expected = Complex::new(1.0f64.exp() * 0.157_299_207_050_285_13, 0.0);
        assert!(
            (wofz(Complex::new(0.0, 1.0)) - expected).abs() < 1e-12,
            "{expected:?}"
        );
        // And the symmetry that comes with it: mirroring through the
        // origin conjugates the answer, wofz(-conj(z)) = conj(wofz(z)).
        let z = Complex::new(0.7, 1.3);
        let value = wofz(z);
        let expected = Complex::new(value.re, -value.im);
        let mirrored = wofz(Complex::new(-z.re, z.im));
        assert!((mirrored - expected).abs() < 1e-12, "{expected:?}");
    }
}
