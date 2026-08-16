//! The golden-image harness (T0.3).
//!
//! Every registered case is rendered twice — once with caching enabled and
//! once with `bypass_caches: true` — and the two results must be byte-equal.
//! That is invariant I-6, the prime invariant of Doc 03 §2, and it is checked
//! before the reference comparison so a caching bug is never mistaken for a
//! rasterizer bug.
//!
//! Only then is the result compared against the stored reference PNG, byte for
//! byte. There is no tolerance: a golden case that needs one is not a golden
//! case (Doc 01 §6 reserves tolerance for cross-backend parity).
//!
//! # Registering a case
//!
//! ```no_run
//! use otf_2d_engine_testing::golden::{GoldenCase, GoldenSuite};
//! use otf_2d_engine_testing::image::Image;
//!
//! fn solid_red(_bypass_caches: bool) -> Result<Image, String> {
//!     Ok(Image::new(16, 16))
//! }
//!
//! let mut suite = GoldenSuite::new("tests/golden");
//! suite.register(GoldenCase::new("solid_red", solid_red));
//! suite.run_or_panic();
//! ```
//!
//! # Updating a reference
//!
//! `OTF_BLESS=1 cargo test -p otf-2d-engine-testing` rewrites references.
//! Per `AGENTS.md`, doing so requires an explicit note in the commit message
//! explaining why the new output is correct. The harness prints a banner
//! saying so, loudly, every time it blesses anything.

use std::fmt;
use std::path::PathBuf;

use crate::image::{Image, Mismatch};

/// Renders one case. `bypass_caches` maps straight onto
/// `RenderParams::bypass_caches`.
pub type RenderFn = fn(bypass_caches: bool) -> Result<Image, String>;

/// One registered golden-image case.
pub struct GoldenCase {
    name: &'static str,
    render: RenderFn,
}

impl GoldenCase {
    pub fn new(name: &'static str, render: RenderFn) -> Self {
        assert!(!name.is_empty(), "golden case name must not be empty");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "golden case name {name:?} must be [A-Za-z0-9_-]+; it becomes a file name"
        );
        Self { name, render }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// What happened to one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseOutcome {
    /// Matched the stored reference, and cached output matched bypassed output.
    Pass,
    /// The reference was written or rewritten because `OTF_BLESS` was set.
    Blessed { existed: bool },
    /// Rendering with caches enabled disagreed with rendering bypassed.
    /// Invariant I-6. This is a release blocker, never a fixture problem.
    CacheDivergence { mismatch: Mismatch },
    /// No reference PNG exists and blessing was not requested.
    MissingReference { path: PathBuf },
    /// The reference exists but could not be read.
    UnreadableReference { path: PathBuf, error: String },
    /// The render function returned an error.
    RenderFailed { error: String },
    /// Output differs from the stored reference.
    Mismatch { mismatch: Mismatch },
}

impl CaseOutcome {
    pub fn is_failure(&self) -> bool {
        !matches!(self, Self::Pass | Self::Blessed { .. })
    }
}

/// The result of one case, with the artefact paths written on failure.
#[derive(Debug, Clone)]
pub struct CaseReport {
    pub name: String,
    pub outcome: CaseOutcome,
    /// Where the harness wrote the actual output, when it wrote one.
    pub artifacts: Vec<PathBuf>,
}

impl fmt::Display for CaseReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.outcome {
            CaseOutcome::Pass => write!(f, "ok      {}", self.name),
            CaseOutcome::Blessed { existed } => {
                let verb = if *existed { "rewrote" } else { "created" };
                write!(f, "BLESSED {} ({verb} reference)", self.name)
            }
            CaseOutcome::CacheDivergence { mismatch } => write!(
                f,
                "FAILED  {} — I-6 VIOLATION: cached output differs from \
                 bypass_caches output: {mismatch}",
                self.name
            ),
            CaseOutcome::MissingReference { path } => write!(
                f,
                "FAILED  {} — no reference at {}; run with OTF_BLESS=1 to create it",
                self.name,
                path.display()
            ),
            CaseOutcome::UnreadableReference { path, error } => write!(
                f,
                "FAILED  {} — reference {} could not be read: {error}",
                self.name,
                path.display()
            ),
            CaseOutcome::RenderFailed { error } => {
                write!(
                    f,
                    "FAILED  {} — render returned an error: {error}",
                    self.name
                )
            }
            CaseOutcome::Mismatch { mismatch } => {
                write!(f, "FAILED  {} — {mismatch}", self.name)
            }
        }?;
        for path in &self.artifacts {
            write!(f, "\n            wrote {}", path.display())?;
        }
        Ok(())
    }
}

/// Everything that happened in one run of the suite.
#[derive(Debug, Clone, Default)]
pub struct SuiteReport {
    pub reports: Vec<CaseReport>,
}

impl SuiteReport {
    pub fn passed(&self) -> usize {
        self.reports
            .iter()
            .filter(|r| matches!(r.outcome, CaseOutcome::Pass))
            .count()
    }

    pub fn blessed(&self) -> usize {
        self.reports
            .iter()
            .filter(|r| matches!(r.outcome, CaseOutcome::Blessed { .. }))
            .count()
    }

    pub fn failures(&self) -> impl Iterator<Item = &CaseReport> {
        self.reports.iter().filter(|r| r.outcome.is_failure())
    }

    pub fn failed(&self) -> usize {
        self.failures().count()
    }
}

impl fmt::Display for SuiteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for report in &self.reports {
            writeln!(f, "{report}")?;
        }
        write!(
            f,
            "\n{} case(s): {} passed, {} blessed, {} failed",
            self.reports.len(),
            self.passed(),
            self.blessed(),
            self.failed()
        )
    }
}

/// A registry of golden cases sharing one reference directory.
pub struct GoldenSuite {
    reference_dir: PathBuf,
    failure_dir: PathBuf,
    cases: Vec<GoldenCase>,
    bless: bool,
}

impl GoldenSuite {
    /// Creates a suite reading references from `reference_dir`.
    ///
    /// Blessing is enabled when `OTF_BLESS` is set to anything but `0`.
    pub fn new(reference_dir: impl Into<PathBuf>) -> Self {
        let bless = std::env::var_os("OTF_BLESS").is_some_and(|v| v != "0");
        Self {
            reference_dir: reference_dir.into(),
            failure_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target/golden-failures"),
            cases: Vec::new(),
            bless,
        }
    }

    /// Overrides where failure artefacts are written.
    pub fn failure_dir(&mut self, dir: impl Into<PathBuf>) -> &mut Self {
        self.failure_dir = dir.into();
        self
    }

    /// Forces blessing on or off, ignoring `OTF_BLESS`. For harness self-tests.
    pub fn set_bless(&mut self, bless: bool) -> &mut Self {
        self.bless = bless;
        self
    }

    pub fn register(&mut self, case: GoldenCase) -> &mut Self {
        assert!(
            !self.cases.iter().any(|c| c.name == case.name),
            "golden case {:?} registered twice",
            case.name
        );
        self.cases.push(case);
        self
    }

    pub fn len(&self) -> usize {
        self.cases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    fn reference_path(&self, name: &str) -> PathBuf {
        self.reference_dir.join(format!("{name}.png"))
    }

    /// Runs every registered case and returns the report without panicking.
    pub fn run(&self) -> SuiteReport {
        let reports = self.cases.iter().map(|case| self.run_case(case)).collect();
        SuiteReport { reports }
    }

    /// Runs every case, printing the report, and panics if any failed.
    ///
    /// Zero registered cases is a pass: the harness must be installable before
    /// there is anything to render (T0.3).
    pub fn run_or_panic(&self) {
        let report = self.run();
        println!("{report}");
        if report.blessed() > 0 {
            println!(
                "\n\
                 ============================================================\n\
                 {} golden reference(s) were REWRITTEN.\n\
                 AGENTS.md: updating a reference image requires an explicit\n\
                 note in the commit message explaining why the new output is\n\
                 correct. Never bless to make a test pass.\n\
                 ============================================================",
                report.blessed()
            );
        }
        if report.failed() > 0 {
            panic!("{} golden case(s) failed:\n{report}", report.failed());
        }
    }

    fn run_case(&self, case: &GoldenCase) -> CaseReport {
        let mut artifacts = Vec::new();
        let finish = |outcome: CaseOutcome, artifacts: Vec<PathBuf>| CaseReport {
            name: case.name.to_string(),
            outcome,
            artifacts,
        };

        // Prime invariant first: a caching bug must not be reported as a
        // rasterizer bug.
        let cached = match (case.render)(false) {
            Ok(img) => img,
            Err(error) => return finish(CaseOutcome::RenderFailed { error }, artifacts),
        };
        let bypassed = match (case.render)(true) {
            Ok(img) => img,
            Err(error) => return finish(CaseOutcome::RenderFailed { error }, artifacts),
        };

        if let Some(mismatch) = cached.compare(&bypassed) {
            artifacts.extend(self.write_artifacts(case.name, &cached, Some(&bypassed), "bypassed"));
            return finish(CaseOutcome::CacheDivergence { mismatch }, artifacts);
        }

        let reference_path = self.reference_path(case.name);
        if self.bless {
            let existed = reference_path.exists();
            if let Err(e) = cached.write_png(&reference_path) {
                return finish(
                    CaseOutcome::UnreadableReference {
                        path: reference_path,
                        error: e.to_string(),
                    },
                    artifacts,
                );
            }
            return finish(CaseOutcome::Blessed { existed }, artifacts);
        }

        if !reference_path.exists() {
            artifacts.extend(self.write_artifacts(case.name, &cached, None, "expected"));
            return finish(
                CaseOutcome::MissingReference {
                    path: reference_path,
                },
                artifacts,
            );
        }

        let reference = match Image::read_png(&reference_path) {
            Ok(img) => img,
            Err(e) => {
                return finish(
                    CaseOutcome::UnreadableReference {
                        path: reference_path,
                        error: e.to_string(),
                    },
                    artifacts,
                );
            }
        };

        match cached.compare(&reference) {
            None => finish(CaseOutcome::Pass, artifacts),
            Some(mismatch) => {
                artifacts.extend(self.write_artifacts(
                    case.name,
                    &cached,
                    Some(&reference),
                    "expected",
                ));
                finish(CaseOutcome::Mismatch { mismatch }, artifacts)
            }
        }
    }

    /// Writes `<name>.actual.png`, `<name>.<label>.png` and a diff overlay so a
    /// failure can be inspected without re-running anything.
    fn write_artifacts(
        &self,
        name: &str,
        actual: &Image,
        other: Option<&Image>,
        label: &str,
    ) -> Vec<PathBuf> {
        let mut written = Vec::new();
        let actual_path = self.failure_dir.join(format!("{name}.actual.png"));
        if actual.write_png(&actual_path).is_ok() {
            written.push(actual_path);
        }
        if let Some(other) = other {
            let other_path = self.failure_dir.join(format!("{name}.{label}.png"));
            if other.write_png(&other_path).is_ok() {
                written.push(other_path);
            }
            if let Some(diff) = actual.diff_image(other) {
                let diff_path = self.failure_dir.join(format!("{name}.diff.png"));
                if diff.write_png(&diff_path).is_ok() {
                    written.push(diff_path);
                }
            }
        }
        written
    }
}
