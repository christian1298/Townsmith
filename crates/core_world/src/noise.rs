//! Small deterministic value-noise implementation.
//!
//! The generator deliberately has no external noise dependency: the same
//! seed and coordinates must produce the same world on every platform.

/// SplitMix64 finalizer.
#[must_use]
fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn hash2d(x: i64, z: i64, seed: u64) -> u64 {
    let ux = x as u64;
    let uz = z as u64;
    splitmix64(seed ^ splitmix64(ux.wrapping_mul(0x9E37_79B9) ^ uz))
}

fn lattice(x: i64, z: i64, seed: u64) -> f32 {
    let bits = hash2d(x, z, seed) >> 40;
    bits as f32 / ((1u64 << 24) as f32)
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// 2D value noise in roughly `[-1, 1]`.
#[must_use]
pub fn value_noise2d(x: f32, z: f32, seed: u64) -> f32 {
    let x0 = x.floor() as i64;
    let z0 = z.floor() as i64;
    let fx = fade(x - x0 as f32);
    let fz = fade(z - z0 as f32);

    let n00 = lattice(x0, z0, seed);
    let n10 = lattice(x0 + 1, z0, seed);
    let n01 = lattice(x0, z0 + 1, seed);
    let n11 = lattice(x0 + 1, z0 + 1, seed);

    let nx0 = lerp(n00, n10, fx);
    let nx1 = lerp(n01, n11, fx);
    lerp(nx0, nx1, fz) * 2.0 - 1.0
}

/// Fractal Brownian motion built from value noise.
#[must_use]
pub fn fbm2d(x: f32, z: f32, seed: u64, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    let mut frequency = 1.0;
    let mut amplitude = 1.0;
    let mut sum = 0.0;
    let mut normalization = 0.0;

    for octave in 0..octaves {
        let octave_seed = seed.wrapping_add(u64::from(octave).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        sum += value_noise2d(x * frequency, z * frequency, octave_seed) * amplitude;
        normalization += amplitude;
        frequency *= lacunarity;
        amplitude *= gain;
    }

    if normalization == 0.0 {
        0.0
    } else {
        sum / normalization
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic() {
        assert_eq!(
            value_noise2d(12.34, -56.78, 42),
            value_noise2d(12.34, -56.78, 42)
        );
        assert_eq!(
            fbm2d(1.5, -2.5, 7, 4, 2.0, 0.5),
            fbm2d(1.5, -2.5, 7, 4, 2.0, 0.5)
        );
    }

    #[test]
    fn different_seeds_differ() {
        let a = fbm2d(10.5, 20.5, 1, 4, 2.0, 0.5);
        let b = fbm2d(10.5, 20.5, 2, 4, 2.0, 0.5);
        assert!((a - b).abs() > f32::EPSILON);
    }

    #[test]
    fn noise_stays_in_range() {
        for z in -10..10 {
            for x in -10..10 {
                let value = fbm2d(x as f32 * 0.37, z as f32 * 0.37, 99, 5, 2.0, 0.5);
                assert!((-1.0..=1.0).contains(&value), "noise out of range: {value}");
            }
        }
    }

    #[test]
    fn negative_coordinates_are_supported() {
        let value = value_noise2d(-12.3, -4.5, 3);
        assert!(value.is_finite());
    }
}
