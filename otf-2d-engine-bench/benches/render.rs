//! The 2D-Engine benchmark corpus (T0.4).
//!
//! `cargo bench` runs every registered benchmark, writes
//! `target/bench-results.json`, and compares it against the tracked
//! `benchmarks/baseline.json`. A regression beyond the threshold exits
//! non-zero. `AGENTS.md` forbids raising the threshold to make that pass; the
//! way to clear a regression is to make it not be one, or to re-record the
//! baseline deliberately with `OTF_BLESS_BENCH=1` and say why in the commit.
//!
//! Benchmarks are registered in `register_all`.

use std::process::ExitCode;

use criterion::Criterion;
use otf_2d_engine_bench::{Registry, baseline_path, fill, repo_root, results_path};
use otf_2d_engine_testing::bench::{BenchResults, threshold_from_env};

/// Every benchmark in the corpus. Add here and nowhere else.
fn register_all(criterion: &mut Criterion, registry: &mut Registry) {
    fill::register(criterion, registry);
    // M3 and M4 add geometry, paint and text groups here.
}

fn main() -> ExitCode {
    let mut criterion = Criterion::default().configure_from_args();
    let mut registry = Registry::new();

    register_all(&mut criterion, &mut registry);
    criterion.final_summary();

    match report(&registry) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("benchmark harness: {message}");
            ExitCode::FAILURE
        }
    }
}

fn report(registry: &Registry) -> Result<ExitCode, String> {
    let results = registry.collect(&repo_root().join("target/criterion"))?;

    let results_path = results_path();
    results
        .save(&results_path)
        .map_err(|e| format!("writing {}: {e}", results_path.display()))?;
    println!(
        "\nwrote {} ({} benchmark(s))",
        results_path.display(),
        results.len()
    );

    let baseline_path = baseline_path();
    if std::env::var_os("OTF_BLESS_BENCH").is_some_and(|v| v != "0") {
        results
            .save(&baseline_path)
            .map_err(|e| format!("writing {}: {e}", baseline_path.display()))?;
        println!(
            "\n\
             ============================================================\n\
             RE-RECORDED {} with {} benchmark(s).\n\
             AGENTS.md: a baseline is re-recorded deliberately, with the\n\
             reason in the commit message. It is not a way to clear a\n\
             regression.\n\
             ============================================================",
            baseline_path.display(),
            results.len()
        );
        return Ok(ExitCode::SUCCESS);
    }

    if !baseline_path.exists() {
        return Err(format!(
            "no baseline at {}; record one with OTF_BLESS_BENCH=1 cargo bench",
            baseline_path.display()
        ));
    }

    let baseline = BenchResults::load(&baseline_path)
        .map_err(|e| format!("reading {}: {e}", baseline_path.display()))?;
    let comparison = results.compare(&baseline, threshold_from_env());
    println!("\n{comparison}");

    if comparison.failed() > 0 {
        eprintln!(
            "\nbenchmark regression against {}:",
            baseline_path.display()
        );
        for failure in comparison.failures() {
            eprintln!("  {failure}");
        }
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}
