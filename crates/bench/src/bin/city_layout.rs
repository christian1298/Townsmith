//! Synthetic city layout generation benchmark.
//!
//! Generates the Early, Mid, and Late layouts that feed the Godot MultiMesh
//! rendering benchmark. The JSON output reports building counts, bounds,
//! generation time, and a layout checksum so accidental layout changes are
//! visible in benchmark logs.

use std::hint::black_box;
use std::time::Instant;

use core_city::{CityLayout, CityTier};

/// Fixed seed for comparable runs.
const SEED: u64 = 0x5EED_0000_0000_0001;

fn main() {
    let mut tiers = Vec::with_capacity(CityTier::ALL.len());

    for tier in CityTier::ALL {
        let start = Instant::now();
        let layout = CityLayout::generate(tier, SEED).expect("synthetic city layout is valid");
        let generate_ms = start.elapsed().as_secs_f64() * 1000.0;
        let checksum = black_box(layout.checksum());
        let [min_x, min_z, max_x, max_z] = layout.bounds_m;

        tiers.push(format!(
            concat!(
                "{{\"tier\":\"{}\",\"population\":{},\"buildings\":{},",
                "\"bounds\":[{},{},{},{}],\"generate_ms\":{:.4},",
                "\"checksum\":{}}}"
            ),
            tier.as_str(),
            layout.population,
            layout.building_count(),
            min_x,
            min_z,
            max_x,
            max_z,
            generate_ms,
            checksum
        ));
    }

    println!(
        "{{\"scene\":\"city_layout\",\"seed\":{},\"tiers\":[{}]}}",
        SEED,
        tiers.join(",")
    );
}
