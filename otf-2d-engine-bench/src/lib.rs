//! Shared plumbing for the 2D-Engine benchmark corpus.
//!
//! Criterion measures; this crate remembers. Every benchmark registers its
//! Criterion identifier here so the run can locate Criterion's own estimate
//! files afterwards and fold them into the tracked JSON baseline that
//! `AGENTS.md` makes a per-commit gate.
#![forbid(unsafe_code)]

pub mod fill;
pub mod stroke;

use std::path::{Path, PathBuf};

use otf_2d_engine_testing::bench::{BenchRecord, BenchResults, STAGE_COUNT};

/// The repository root, derived from this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate has a parent dir")
        .to_path_buf()
}

/// The tracked baseline. Committed to git; regressions against it fail CI.
pub fn baseline_path() -> PathBuf {
    repo_root().join("benchmarks/baseline.json")
}

/// Where this run's measurements are written.
pub fn results_path() -> PathBuf {
    repo_root().join("target/bench-results.json")
}

/// One benchmark's identity plus any per-stage timings it captured.
pub struct Registration {
    /// Criterion group name, e.g. `fill`.
    pub group: String,
    /// Criterion benchmark id within the group, e.g. `solid_rect_1080p`.
    pub id: String,
    /// Per-stage nanoseconds from `RenderStats`, or all zero when the
    /// benchmark does not run a full render.
    pub stage_nanos: [f64; STAGE_COUNT],
}

/// Accumulates the benchmarks a run registered.
#[derive(Default)]
pub struct Registry {
    registrations: Vec<Registration>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `group/id` was benchmarked.
    pub fn record(&mut self, group: impl Into<String>, id: impl Into<String>) {
        self.record_with_stages(group, id, [0.0; STAGE_COUNT]);
    }

    /// Records `group/id` together with per-stage timings from `RenderStats`.
    pub fn record_with_stages(
        &mut self,
        group: impl Into<String>,
        id: impl Into<String>,
        stage_nanos: [f64; STAGE_COUNT],
    ) {
        self.registrations.push(Registration {
            group: group.into(),
            id: id.into(),
            stage_nanos,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.registrations.len()
    }

    /// Folds Criterion's mean estimates into a [`BenchResults`].
    ///
    /// A registered benchmark with no estimate file is an error rather than a
    /// silent omission — that is exactly how a regression gate stops gating.
    pub fn collect(&self, criterion_dir: &Path) -> Result<BenchResults, String> {
        let mut results = BenchResults::default();
        for reg in &self.registrations {
            let name = format!("{}/{}", reg.group, reg.id);
            let estimates = criterion_dir
                .join(sanitize(&reg.group))
                .join(sanitize(&reg.id))
                .join("new/estimates.json");
            let mean = read_mean_nanos(&estimates).map_err(|e| {
                format!(
                    "benchmark {name} registered but {} unusable: {e}",
                    estimates.display()
                )
            })?;
            results.insert(BenchRecord::new(name, mean).with_stage_nanos(reg.stage_nanos));
        }
        Ok(results)
    }
}

/// Criterion replaces path-hostile characters when it builds directory names.
fn sanitize(component: &str) -> String {
    component
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_. ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Pulls `mean.point_estimate` out of a Criterion estimates file.
fn read_mean_nanos(path: &Path) -> Result<f64, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    value
        .get("mean")
        .and_then(|m| m.get("point_estimate"))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "no mean.point_estimate".to_string())
}
