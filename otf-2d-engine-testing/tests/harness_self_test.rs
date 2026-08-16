//! T0.3 requires proving the harness fails loudly on a divergent case. Rather
//! than corrupting a fixture by hand once and deleting it, these tests keep
//! that proof permanent: each failure mode is exercised against a private
//! scratch reference directory.

use otf_2d_engine_testing::golden::{CaseOutcome, GoldenCase, GoldenSuite};
use otf_2d_engine_testing::image::Image;
use otf_2d_engine_testing::scratch_dir;

/// A flat 8x8 image in the given colour.
fn flat(rgba: [u8; 4]) -> Image {
    let mut img = Image::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            img.set_pixel(x, y, rgba);
        }
    }
    img
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn stable(_bypass: bool) -> Result<Image, String> {
    Ok(flat(RED))
}

/// Renders differently depending on `bypass_caches` — an I-6 violation.
fn cache_divergent(bypass: bool) -> Result<Image, String> {
    Ok(flat(if bypass { BLUE } else { RED }))
}

fn failing(_bypass: bool) -> Result<Image, String> {
    Err("deliberate failure".to_string())
}

/// Differs from `stable` in exactly one pixel.
fn one_pixel_off(_bypass: bool) -> Result<Image, String> {
    let mut img = flat(RED);
    img.set_pixel(3, 4, BLUE);
    Ok(img)
}

fn suite_in(dir: &std::path::Path) -> GoldenSuite {
    let mut suite = GoldenSuite::new(dir);
    suite.set_bless(false).failure_dir(dir.join("failures"));
    suite
}

#[test]
fn empty_suite_passes() {
    let dir = scratch_dir("golden_empty");
    let suite = suite_in(&dir);
    assert!(suite.is_empty());
    let report = suite.run();
    assert_eq!(report.failed(), 0);
    // T0.3: zero registered cases must exit 0, not error on an empty corpus.
    suite.run_or_panic();
}

#[test]
fn blessing_creates_then_matches_a_reference() {
    let dir = scratch_dir("golden_bless");

    let mut blessing = GoldenSuite::new(&dir);
    blessing
        .set_bless(true)
        .register(GoldenCase::new("stable", stable));
    let report = blessing.run();
    assert_eq!(report.blessed(), 1);
    assert_eq!(
        report.reports[0].outcome,
        CaseOutcome::Blessed { existed: false }
    );
    assert!(dir.join("stable.png").exists());

    let mut checking = suite_in(&dir);
    checking.register(GoldenCase::new("stable", stable));
    let report = checking.run();
    assert_eq!(report.reports[0].outcome, CaseOutcome::Pass, "{report}");
}

#[test]
fn a_corrupted_reference_is_detected_and_artifacts_are_written() {
    let dir = scratch_dir("golden_corrupt");

    let mut blessing = GoldenSuite::new(&dir);
    blessing
        .set_bless(true)
        .register(GoldenCase::new("case", stable));
    blessing.run();

    // Deliberately corrupt the fixture: one pixel, the smallest possible lie.
    let mut checking = suite_in(&dir);
    checking.register(GoldenCase::new("case", one_pixel_off));
    let report = checking.run();

    assert_eq!(report.failed(), 1, "{report}");
    let CaseOutcome::Mismatch { mismatch } = &report.reports[0].outcome else {
        panic!("expected a mismatch, got {:?}", report.reports[0].outcome);
    };
    assert_eq!(mismatch.differing_pixels, 1);
    assert_eq!(mismatch.max_channel_delta, 255);
    let first = mismatch.first_difference.expect("a located difference");
    assert_eq!((first.x, first.y), (3, 4));
    assert_eq!(first.actual, BLUE);
    assert_eq!(first.expected, RED);

    // Actual, expected and diff must be on disk for inspection.
    assert_eq!(report.reports[0].artifacts.len(), 3, "{report}");
    for path in &report.reports[0].artifacts {
        assert!(
            path.exists(),
            "{} was reported but not written",
            path.display()
        );
    }
}

#[test]
fn cache_divergence_is_reported_before_the_reference_comparison() {
    let dir = scratch_dir("golden_divergence");

    // A correct reference exists, so the only fault is the I-6 violation.
    let mut blessing = GoldenSuite::new(&dir);
    blessing
        .set_bless(true)
        .register(GoldenCase::new("case", stable));
    blessing.run();

    let mut checking = suite_in(&dir);
    checking.register(GoldenCase::new("case", cache_divergent));
    let report = checking.run();

    assert_eq!(report.failed(), 1, "{report}");
    let CaseOutcome::CacheDivergence { mismatch } = &report.reports[0].outcome else {
        panic!(
            "expected cache divergence, got {:?}",
            report.reports[0].outcome
        );
    };
    assert_eq!(mismatch.differing_pixels, 64);
    assert!(report.to_string().contains("I-6 VIOLATION"), "{report}");
}

#[test]
fn a_missing_reference_fails_rather_than_silently_passing() {
    let dir = scratch_dir("golden_missing");
    let mut suite = suite_in(&dir);
    suite.register(GoldenCase::new("absent", stable));
    let report = suite.run();
    assert_eq!(report.failed(), 1, "{report}");
    assert!(matches!(
        report.reports[0].outcome,
        CaseOutcome::MissingReference { .. }
    ));
    assert!(report.to_string().contains("OTF_BLESS=1"), "{report}");
}

#[test]
fn a_render_error_fails_the_case() {
    let dir = scratch_dir("golden_render_error");
    let mut suite = suite_in(&dir);
    suite.register(GoldenCase::new("broken", failing));
    let report = suite.run();
    assert_eq!(report.failed(), 1, "{report}");
    assert!(matches!(
        report.reports[0].outcome,
        CaseOutcome::RenderFailed { .. }
    ));
}

#[test]
#[should_panic(expected = "golden case(s) failed")]
fn run_or_panic_panics_on_failure() {
    let dir = scratch_dir("golden_panic");
    let mut suite = suite_in(&dir);
    suite.register(GoldenCase::new("absent", stable));
    suite.run_or_panic();
}

#[test]
#[should_panic(expected = "registered twice")]
fn duplicate_registration_is_rejected() {
    let dir = scratch_dir("golden_duplicate");
    let mut suite = suite_in(&dir);
    suite.register(GoldenCase::new("dup", stable));
    suite.register(GoldenCase::new("dup", stable));
}

#[test]
fn png_round_trips_through_disk() {
    let dir = scratch_dir("png_round_trip");
    let mut img = Image::new(5, 3);
    for y in 0..3 {
        for x in 0..5 {
            img.set_pixel(x, y, [x as u8 * 50, y as u8 * 80, 7, 128 + x as u8]);
        }
    }
    let path = dir.join("round_trip.png");
    img.write_png(&path).expect("write");
    let read = Image::read_png(&path).expect("read");
    assert_eq!(read, img);
    assert_eq!(img.compare(&read), None);
}
