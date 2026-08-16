//! The golden-image corpus.
//!
//! Cases are registered here explicitly — no macro registry, no link-time
//! collection, so the corpus is a list you can read. Cases land from T2.6
//! onward; until then the suite is empty and passing, which is exactly what
//! T0.3 requires.

use otf_2d_engine_testing::golden::GoldenSuite;

fn suite() -> GoldenSuite {
    let mut suite = GoldenSuite::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden"));

    // ---- Registered cases ----
    // T2.6 registers the first ≥20 solid-fill cases here.
    let _ = &mut suite;

    suite
}

#[test]
fn golden_corpus() {
    suite().run_or_panic();
}
