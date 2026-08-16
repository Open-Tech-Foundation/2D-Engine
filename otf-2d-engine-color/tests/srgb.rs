//! T1.2 transfer-function criteria.

use otf_2d_engine_color::{
    Color, ColorSpace, alpha8_to_f32, f32_to_alpha8, linear_to_srgb, linear_to_srgb8,
    srgb_to_linear, srgb8_to_linear,
};
use proptest::prelude::*;

/// The T1.2 criterion, exhaustively: `srgb8 -> linear -> srgb8` is the
/// identity for all 256³ colours. Ignored by default because it is 16.7M
/// conversions; run with `cargo test -- --ignored`.
#[test]
#[ignore = "exhaustive: 256^3 conversions"]
fn srgb8_round_trips_exactly_for_all_16_777_216_colours() {
    for r in 0..=255u8 {
        for g in 0..=255u8 {
            for b in 0..=255u8 {
                let round_trip = Color::from_srgb8(r, g, b, 255).to_srgb8();
                assert_eq!(
                    round_trip,
                    [r, g, b, 255],
                    "sRGB round trip lost ({r}, {g}, {b})"
                );
            }
        }
    }
}

/// The per-channel core of the exhaustive test, cheap enough to run always.
/// If this passes for every channel value then the 256³ product does too,
/// since channels convert independently at full alpha.
#[test]
fn every_8_bit_channel_round_trips_exactly() {
    for v in 0..=255u8 {
        let linear = srgb8_to_linear(v);
        assert_eq!(linear_to_srgb8(linear), v, "channel {v} did not round trip");
    }
}

/// Every 8-bit alpha survives the trip through `f32`, which the exhaustive
/// test's fixed alpha does not cover.
#[test]
fn every_8_bit_alpha_round_trips_exactly() {
    for a in 0..=255u8 {
        assert_eq!(
            f32_to_alpha8(alpha8_to_f32(a)),
            a,
            "alpha {a} did not round trip"
        );
    }
    // White through premultiplication and back, at every non-zero alpha.
    for a in 1..=255u8 {
        assert_eq!(
            Color::from_srgb8(255, 255, 255, a).to_srgb8(),
            [255, 255, 255, a]
        );
    }
}

#[test]
fn a_fully_transparent_colour_has_no_recoverable_hue() {
    // Premultiplied storage multiplies the colour away at alpha 0. This is
    // inherent to the representation Doc 01 §7 chose, not a defect: an
    // invisible colour has no hue to preserve. Pinned so nobody "fixes" it by
    // storing straight alpha alongside.
    assert_eq!(Color::from_srgb8(255, 0, 0, 0).to_srgb8(), [0, 0, 0, 0]);
    assert_eq!(
        Color::from_srgb8(255, 0, 0, 0).to_straight(),
        [0.0, 0.0, 0.0, 0.0]
    );
    assert!(Color::TRANSPARENT.is_transparent());
}

#[test]
fn low_alpha_costs_at_most_one_code_of_colour_precision() {
    // Dividing alpha back out at alpha = 1/255 amplifies rounding. f32
    // storage keeps that inside a single 8-bit code; a u8 premultiplied model
    // would not, which is why f32 is the model and u8 only a fast path.
    for &(r, g, b) in &[(1u8, 2u8, 3u8), (17, 128, 240), (255, 1, 128)] {
        for a in 1..=255u8 {
            let out = Color::from_srgb8(r, g, b, a).to_srgb8();
            assert_eq!(out[3], a);
            for (i, expected) in [r, g, b].iter().enumerate() {
                assert!(
                    out[i].abs_diff(*expected) <= 1,
                    "({r},{g},{b},{a}) came back as {out:?}"
                );
            }
        }
    }
}

/// A representative slice of the 256³ space, run on every commit so a
/// regression does not wait for the ignored exhaustive test.
#[test]
fn a_sampled_grid_of_colours_round_trips_exactly() {
    for r in (0..=255u8).step_by(17) {
        for g in (0..=255u8).step_by(11) {
            for b in (0..=255u8).step_by(7) {
                assert_eq!(Color::from_srgb8(r, g, b, 255).to_srgb8(), [r, g, b, 255]);
            }
        }
    }
}

#[test]
fn the_lookup_table_agrees_with_the_analytic_transfer_function() {
    for v in 0..=255u8 {
        let table = srgb8_to_linear(v);
        let analytic = srgb_to_linear(v as f32 / 255.0);
        // The analytic path goes through f32 `powf`, which is a few ULP off
        // the exactly-rounded table value.
        assert!(
            (table - analytic).abs() <= 1e-6 * analytic.max(1e-3),
            "code {v}: table {table} vs analytic {analytic}"
        );
    }
}

#[test]
fn the_transfer_function_pins_its_endpoints_and_knee() {
    assert_eq!(srgb_to_linear(0.0), 0.0);
    assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    assert_eq!(linear_to_srgb(0.0), 0.0);
    assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
    // The two branches must meet at the knee, or 8-bit codes near it round
    // to the wrong neighbour.
    let knee = 0.040_45;
    let below = srgb_to_linear(knee - 1e-7);
    let above = srgb_to_linear(knee + 1e-7);
    assert!(
        (below - above).abs() < 1e-6,
        "discontinuity at the knee: {below} vs {above}"
    );
}

#[test]
fn mid_grey_is_not_half_way_in_linear_light() {
    // The single most common sRGB mistake, pinned as a test so nobody
    // "simplifies" the transfer function away.
    let linear = srgb8_to_linear(128);
    assert!(
        linear > 0.21 && linear < 0.22,
        "sRGB 128 decoded to {linear}"
    );
}

#[test]
fn out_of_range_values_are_mirrored_rather_than_clamped() {
    // Wide-gamut conversions legitimately produce negative components, and a
    // clamp inside the transfer function would silently destroy them.
    assert!((srgb_to_linear(-0.5) + srgb_to_linear(0.5)).abs() < 1e-9);
    assert!((linear_to_srgb(-0.5) + linear_to_srgb(0.5)).abs() < 1e-9);
    assert!(srgb_to_linear(2.0) > 1.0);
}

#[test]
fn encoding_to_8_bits_clamps_and_rejects_nan() {
    assert_eq!(linear_to_srgb8(-1.0), 0);
    assert_eq!(linear_to_srgb8(2.0), 255);
    // NaN must not fall through a `>=` comparison and become white.
    assert_eq!(linear_to_srgb8(f32::NAN), 0);
    assert_eq!(f32_to_alpha8(f32::NAN), 0);
    assert_eq!(f32_to_alpha8(-1.0), 0);
    assert_eq!(f32_to_alpha8(2.0), 255);
}

proptest! {
    /// `linear -> srgb -> linear` is the identity for the analytic pair.
    #[test]
    fn the_analytic_transfer_functions_are_inverses(v in 0.0f32..1.0) {
        let round_trip = srgb_to_linear(linear_to_srgb(v));
        prop_assert!((round_trip - v).abs() <= 1e-6, "{v} -> {round_trip}");
    }

    /// Encoding is monotonic, so an ordering of linear values survives into
    /// 8-bit sRGB. Dithering and gradients both depend on this.
    #[test]
    fn encoding_is_monotonic(a in 0.0f32..1.0, b in 0.0f32..1.0) {
        prop_assume!(a < b);
        prop_assert!(linear_to_srgb8(a) <= linear_to_srgb8(b));
    }

    /// An opaque colour survives the trip through the model unchanged.
    #[test]
    fn any_opaque_srgb8_colour_round_trips(r: u8, g: u8, b: u8) {
        prop_assert_eq!(Color::from_srgb8(r, g, b, 255).to_srgb8(), [r, g, b, 255]);
    }
}

#[test]
fn a_hex_literal_matches_the_component_constructor() {
    assert_eq!(
        Color::from_rgba8_hex(0x1e1e24ff),
        Color::from_srgb8(0x1e, 0x1e, 0x24, 0xff)
    );
    assert_eq!(Color::from_rgba8_hex(0x00000000), Color::TRANSPARENT);
    assert_eq!(Color::from_rgba8_hex(0xffffffff).space, ColorSpace::Srgb);
}
