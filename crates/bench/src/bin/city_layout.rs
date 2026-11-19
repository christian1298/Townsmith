//! Synthetic city layout generation benchmark.
//!
//! Generates the Early, Mid, and Late layouts that feed the Godot MultiMesh
//! rendering benchmark. The JSON output reports building counts, bounds,
//! generation medians, and a layout checksum so accidental layout changes are
//! visible in benchmark logs.

use std::hint::black_box;
use std::time::Instant;

use core_city::{CityLayout, CityTier};

/// Fixed seed for comparable runs.
const SEED: u64 = 0x5EED_0000_0000_0001;
const WARMUP: usize = 1;
const ITERATIONS: usize = 5;

fn main() {
    let mut tiers = Vec::with_capacity(CityTier::ALL.len());

    for tier in CityTier::ALL {
        let mut samples = Vec::with_capacity(ITERATIONS);
        let mut layout = None;
        for index in 0..WARMUP + ITERATIONS {
            let start = Instant::now();
            let generated =
                CityLayout::generate(tier, SEED).expect("synthetic city layout is valid");
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if index >= WARMUP {
                samples.push(elapsed);
            }
            layout = Some(generated);
        }
        let layout = layout.expect("at least one layout");
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = samples[samples.len() / 2];
        let min = samples.first().copied().unwrap_or_default();
        let max = samples.last().copied().unwrap_or_default();
        let checksum = black_box(layout.checksum());
        let [min_x, min_z, max_x, max_z] = layout.bounds_m;

        tiers.push(format!(
            concat!(
                "{{\"tier\":\"{}\",\"population\":{},\"buildings\":{},",
                "\"bounds\":[{},{},{},{}],\"generate_ms_median\":{:.4},",
                "\"generate_ms_min\":{:.4},\"generate_ms_max\":{:.4},",
                "\"checksum\":\"{}\"}}"
            ),
            tier.as_str(),
            layout.population,
            layout.building_count(),
            min_x,
            min_z,
            max_x,
            max_z,
            median,
            min,
            max,
            checksum
        ));
    }

    println!(
        "{{\"scene\":\"city_layout\",\"seed\":{},\"iterations\":{},\"tiers\":[{}]}}",
        SEED,
        ITERATIONS,
        tiers.join(",")
    );
}
