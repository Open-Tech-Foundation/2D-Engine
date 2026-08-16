//! Benchmark result recording and regression comparison (T0.4).
//!
//! Criterion provides the statistics; this module provides the *memory*. A run
//! writes a machine-readable result file, and that file is compared against a
//! baseline tracked in git. `AGENTS.md` makes a regression beyond threshold a
//! build failure, and explicitly forbids raising the threshold to pass — so
//! the threshold lives here, in code, not in a runner's flags.
//!
//! Per-stage timings come from `RenderStats::stage_timings`, which ships in
//! release builds precisely so this file can exist (Doc 02 §7).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The seven pipeline stages of Doc 01 §3, in order.
pub const STAGE_NAMES: [&str; 7] = [
    "encode", "resolve", "flatten", "bin", "strips", "fine", "compose",
];

/// Number of pipeline stages. Matches `RenderStats::stage_timings`.
pub const STAGE_COUNT: usize = STAGE_NAMES.len();

/// Default regression threshold: a benchmark may not get more than 5% slower.
pub const DEFAULT_THRESHOLD: f64 = 1.05;

/// Bumped whenever the on-disk shape changes, so a stale baseline is a clear
/// error rather than a silent misread.
const SCHEMA_VERSION: u32 = 1;

/// One benchmark's measured result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchRecord {
    /// Stable identifier, e.g. `fill/solid_rect_1080p`.
    pub name: String,
    /// Criterion's point estimate of the mean, in nanoseconds.
    pub wall_nanos: f64,
    /// Per-stage nanoseconds from `RenderStats`, in `STAGE_NAMES` order.
    /// All zero for benchmarks that do not run a full render.
    #[serde(default)]
    pub stage_nanos: [f64; STAGE_COUNT],
}

impl BenchRecord {
    pub fn new(name: impl Into<String>, wall_nanos: f64) -> Self {
        Self {
            name: name.into(),
            wall_nanos,
            stage_nanos: [0.0; STAGE_COUNT],
        }
    }

    pub fn with_stage_nanos(mut self, stage_nanos: [f64; STAGE_COUNT]) -> Self {
        self.stage_nanos = stage_nanos;
        self
    }
}

/// A complete set of benchmark results, keyed by name so the serialised form
/// has a stable order and produces readable diffs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchResults {
    pub schema: u32,
    pub records: BTreeMap<String, BenchRecord>,
}

impl Default for BenchResults {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            records: BTreeMap::new(),
        }
    }
}

/// Why a results file could not be read or written.
#[derive(Debug)]
pub enum BenchError {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// The file was written by a different, incompatible harness version.
    SchemaMismatch {
        found: u32,
        expected: u32,
    },
}

impl std::fmt::Display for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::SchemaMismatch { found, expected } => write!(
                f,
                "baseline schema {found} but this harness writes {expected}; \
                 re-record the baseline"
            ),
        }
    }
}

impl std::error::Error for BenchError {}

impl From<std::io::Error> for BenchError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for BenchError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl BenchResults {
    pub fn insert(&mut self, record: BenchRecord) {
        self.records.insert(record.name.clone(), record);
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, BenchError> {
        let text = std::fs::read_to_string(path.as_ref())?;
        let results: Self = serde_json::from_str(&text)?;
        if results.schema != SCHEMA_VERSION {
            return Err(BenchError::SchemaMismatch {
                found: results.schema,
                expected: SCHEMA_VERSION,
            });
        }
        Ok(results)
    }

    /// Writes pretty-printed JSON with a trailing newline. `BTreeMap` ordering
    /// keeps the output byte-stable so a tracked baseline diffs cleanly.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), BenchError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Compares `self` (current) against `baseline`.
    ///
    /// Benchmarks absent from the baseline are reported as `Added`, not as
    /// failures: a new benchmark has nothing to regress against. Benchmarks
    /// absent from `self` are reported as `Removed`, which *is* a failure —
    /// silently dropping a benchmark is how a regression gate rots.
    pub fn compare(&self, baseline: &BenchResults, threshold: f64) -> Comparison {
        let mut changes = Vec::new();
        for (name, base) in &baseline.records {
            match self.records.get(name) {
                None => changes.push(Change::Removed { name: name.clone() }),
                Some(current) => {
                    let ratio = if base.wall_nanos > 0.0 {
                        current.wall_nanos / base.wall_nanos
                    } else {
                        1.0
                    };
                    changes.push(Change::Measured {
                        name: name.clone(),
                        baseline_nanos: base.wall_nanos,
                        current_nanos: current.wall_nanos,
                        ratio,
                        regressed: ratio > threshold,
                    });
                }
            }
        }
        for name in self.records.keys() {
            if !baseline.records.contains_key(name) {
                changes.push(Change::Added { name: name.clone() });
            }
        }
        changes.sort_by(|a, b| a.name().cmp(b.name()));
        Comparison { threshold, changes }
    }
}

/// What happened to one benchmark relative to the baseline.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Measured {
        name: String,
        baseline_nanos: f64,
        current_nanos: f64,
        ratio: f64,
        regressed: bool,
    },
    /// Present now, absent from the baseline.
    Added { name: String },
    /// Present in the baseline, absent now.
    Removed { name: String },
}

impl Change {
    pub fn name(&self) -> &str {
        match self {
            Self::Measured { name, .. } | Self::Added { name } | Self::Removed { name } => name,
        }
    }

    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::Measured {
                regressed: true,
                ..
            } | Self::Removed { .. }
        )
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Measured {
                name,
                baseline_nanos,
                current_nanos,
                ratio,
                regressed,
            } => {
                let tag = if *regressed { "REGRESSED" } else { "ok       " };
                write!(
                    f,
                    "{tag} {name}: {:.3?} -> {:.3?} ({:+.1}%)",
                    std::time::Duration::from_nanos(*baseline_nanos as u64),
                    std::time::Duration::from_nanos(*current_nanos as u64),
                    (ratio - 1.0) * 100.0
                )
            }
            Self::Added { name } => write!(f, "added     {name} (no baseline yet)"),
            Self::Removed { name } => {
                write!(f, "REMOVED   {name}: in the baseline but not measured")
            }
        }
    }
}

/// The outcome of comparing a run against a baseline.
#[derive(Debug, Clone)]
pub struct Comparison {
    pub threshold: f64,
    pub changes: Vec<Change>,
}

impl Comparison {
    pub fn failures(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.is_failure())
    }

    pub fn failed(&self) -> usize {
        self.failures().count()
    }
}

impl std::fmt::Display for Comparison {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for change in &self.changes {
            writeln!(f, "  {change}")?;
        }
        write!(
            f,
            "{} benchmark(s) compared at a {:.0}% threshold, {} failing",
            self.changes.len(),
            (self.threshold - 1.0) * 100.0,
            self.failed()
        )
    }
}

/// Reads the regression threshold from `OTF_BENCH_THRESHOLD`, falling back to
/// [`DEFAULT_THRESHOLD`]. Loosening it is a reviewable act, not a habit.
pub fn threshold_from_env() -> f64 {
    parse_threshold(std::env::var("OTF_BENCH_THRESHOLD").ok().as_deref())
}

/// Parses a threshold, rejecting anything non-finite or below parity — a
/// "threshold" under 1.0 would demand a speedup on every commit, which is not
/// a gate anyone means to set.
pub fn parse_threshold(value: Option<&str>) -> f64 {
    value
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 1.0)
        .unwrap_or(DEFAULT_THRESHOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(entries: &[(&str, f64)]) -> BenchResults {
        let mut r = BenchResults::default();
        for (name, nanos) in entries {
            r.insert(BenchRecord::new(*name, *nanos));
        }
        r
    }

    #[test]
    fn an_empty_result_set_round_trips_through_disk() {
        let dir = crate::scratch_dir("bench_empty");
        let path = dir.join("baseline.json");
        let empty = BenchResults::default();
        empty.save(&path).expect("save");
        assert_eq!(BenchResults::load(&path).expect("load"), empty);
    }

    #[test]
    fn saved_json_is_byte_stable_regardless_of_insertion_order() {
        let dir = crate::scratch_dir("bench_stable");
        let forward = results(&[("a/one", 10.0), ("b/two", 20.0), ("c/three", 30.0)]);
        let mut backward = BenchResults::default();
        for (name, nanos) in [("c/three", 30.0), ("b/two", 20.0), ("a/one", 10.0)] {
            backward.insert(BenchRecord::new(name, nanos));
        }
        forward.save(dir.join("f.json")).expect("save");
        backward.save(dir.join("b.json")).expect("save");
        assert_eq!(
            std::fs::read(dir.join("f.json")).unwrap(),
            std::fs::read(dir.join("b.json")).unwrap()
        );
    }

    #[test]
    fn saved_json_ends_with_a_newline() {
        let dir = crate::scratch_dir("bench_newline");
        let path = dir.join("baseline.json");
        results(&[("a/one", 1.0)]).save(&path).expect("save");
        assert!(std::fs::read_to_string(&path).unwrap().ends_with("}\n"));
    }

    #[test]
    fn a_slowdown_beyond_the_threshold_fails() {
        let baseline = results(&[("fill/rect", 100.0)]);
        let current = results(&[("fill/rect", 106.0)]);
        let comparison = current.compare(&baseline, DEFAULT_THRESHOLD);
        assert_eq!(comparison.failed(), 1, "{comparison}");
        assert!(comparison.to_string().contains("REGRESSED"), "{comparison}");
    }

    #[test]
    fn a_slowdown_within_the_threshold_passes() {
        let baseline = results(&[("fill/rect", 100.0)]);
        let current = results(&[("fill/rect", 104.0)]);
        assert_eq!(current.compare(&baseline, DEFAULT_THRESHOLD).failed(), 0);
    }

    #[test]
    fn a_speedup_passes() {
        let baseline = results(&[("fill/rect", 100.0)]);
        let current = results(&[("fill/rect", 40.0)]);
        assert_eq!(current.compare(&baseline, DEFAULT_THRESHOLD).failed(), 0);
    }

    #[test]
    fn a_new_benchmark_is_reported_but_does_not_fail() {
        let comparison = results(&[("fill/rect", 100.0)]).compare(&BenchResults::default(), 1.05);
        assert_eq!(comparison.failed(), 0);
        assert!(matches!(comparison.changes[0], Change::Added { .. }));
    }

    #[test]
    fn a_benchmark_that_stopped_being_measured_fails() {
        let baseline = results(&[("fill/rect", 100.0)]);
        let comparison = BenchResults::default().compare(&baseline, 1.05);
        assert_eq!(comparison.failed(), 1, "{comparison}");
        assert!(matches!(comparison.changes[0], Change::Removed { .. }));
    }

    #[test]
    fn a_baseline_from_a_different_schema_is_rejected() {
        let dir = crate::scratch_dir("bench_schema");
        let path = dir.join("baseline.json");
        std::fs::write(&path, r#"{"schema": 999, "records": {}}"#).unwrap();
        assert!(matches!(
            BenchResults::load(&path),
            Err(BenchError::SchemaMismatch { found: 999, .. })
        ));
    }

    #[test]
    fn the_threshold_rejects_nonsense_and_anything_below_parity() {
        assert_eq!(parse_threshold(None), DEFAULT_THRESHOLD);
        assert_eq!(parse_threshold(Some("not a number")), DEFAULT_THRESHOLD);
        assert_eq!(parse_threshold(Some("nan")), DEFAULT_THRESHOLD);
        assert_eq!(parse_threshold(Some("inf")), DEFAULT_THRESHOLD);
        // A threshold below parity would demand a speedup every commit.
        assert_eq!(parse_threshold(Some("0.5")), DEFAULT_THRESHOLD);
        assert_eq!(parse_threshold(Some(" 1.20 ")), 1.20);
        assert_eq!(parse_threshold(Some("1.0")), 1.0);
    }

    #[test]
    fn stage_names_match_the_seven_pipeline_stages() {
        assert_eq!(STAGE_COUNT, 7);
        assert_eq!(STAGE_NAMES[0], "encode");
        assert_eq!(STAGE_NAMES[STAGE_COUNT - 1], "compose");
    }
}
