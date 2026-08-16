//! T1.2 model criteria: premultiply round-trips within 1 ULP for alpha > 0,
//! and colour space is data on the colour rather than a global.

use otf_2d_engine_color::{
    BlendMode, Color, ColorSpace, apply_coverage, apply3, invert3, src_over, src_over_premul,
};
use proptest::prelude::*;
use static_assertions::assert_impl_all;

assert_impl_all!(Color: Send, Sync, Copy);
assert_impl_all!(ColorSpace: Send, Sync, Copy);
assert_impl_all!(BlendMode: Send, Sync, Copy);

/// Units in the last place between two `f32`s, for values of the same sign.
fn ulps_between(a: f32, b: f32) -> u32 {
    if a == b {
        return 0;
    }
    let (ia, ib) = (a.to_bits() as i64, b.to_bits() as i64);
    (ia - ib).unsigned_abs().min(u32::MAX as u64) as u32
}

proptest! {
    /// The T1.2 criterion: premultiply then unpremultiply is within 1 ULP for
    /// any alpha above zero.
    #[test]
    fn premultiply_round_trips_within_one_ulp(
        r in 0.0f32..=1.0, g in 0.0f32..=1.0, b in 0.0f32..=1.0, a in f32::MIN_POSITIVE..=1.0,
    ) {
        let round_trip = Color::from_rgba_f32(r, g, b, a).to_straight();
        for (i, original) in [r, g, b].iter().enumerate() {
            prop_assert!(
                ulps_between(round_trip[i], *original) <= 1,
                "channel {i}: {} vs {original} ({} ULP)",
                round_trip[i], ulps_between(round_trip[i], *original)
            );
        }
        prop_assert_eq!(round_trip[3], a, "alpha must be exact");
    }

    /// Premultiplied components never exceed alpha for in-gamut input, which
    /// is the invariant the fine rasterizer relies on.
    #[test]
    fn premultiplied_components_never_exceed_alpha(
        r in 0.0f32..=1.0, g in 0.0f32..=1.0, b in 0.0f32..=1.0, a in 0.0f32..=1.0,
    ) {
        let c = Color::from_rgba_f32(r, g, b, a);
        prop_assert!(c.is_valid_premul(), "{c:?}");
    }

    /// Group opacity is a single scale of all four channels.
    #[test]
    fn alpha_multiplication_scales_every_channel(
        r in 0.0f32..=1.0, g in 0.0f32..=1.0, b in 0.0f32..=1.0,
        a in 0.0f32..=1.0, factor in 0.0f32..=1.0,
    ) {
        let c = Color::from_rgba_f32(r, g, b, a);
        let scaled = c.with_alpha_multiplied(factor);
        prop_assert!(scaled.is_valid_premul(), "{scaled:?}");
        // The straight colour is unchanged; only the alpha moved.
        if scaled.a > 1e-6 {
            let before = c.to_straight();
            let after = scaled.to_straight();
            for i in 0..3 {
                prop_assert!((before[i] - after[i]).abs() < 1e-5, "{before:?} vs {after:?}");
            }
        }
    }

    /// Compositing anything over an opaque destination leaves it opaque.
    #[test]
    fn src_over_preserves_an_opaque_destination(
        sr in 0.0f32..=1.0, sg in 0.0f32..=1.0, sb in 0.0f32..=1.0, sa in 0.0f32..=1.0,
    ) {
        let src = Color::from_rgba_f32(sr, sg, sb, sa);
        let dst = Color::from_rgba_f32(0.2, 0.4, 0.6, 1.0);
        let out = src_over(src, dst);
        prop_assert!((out.a - 1.0).abs() < 1e-6, "alpha became {}", out.a);
        prop_assert!(out.is_valid_premul(), "{out:?}");
    }

    /// A colour survives a round trip through another colour space.
    #[test]
    fn colour_space_conversion_round_trips(
        r in 0.0f32..=1.0, g in 0.0f32..=1.0, b in 0.0f32..=1.0, a in 0.01f32..=1.0,
    ) {
        for space in [ColorSpace::DisplayP3, ColorSpace::Rec2020] {
            let original = Color::from_rgba_f32(r, g, b, a);
            let round_trip = original.convert_to(space).convert_to(ColorSpace::Srgb);
            for i in 0..4 {
                prop_assert!(
                    (round_trip.to_premul()[i] - original.to_premul()[i]).abs() <= 1e-4,
                    "{space:?}: {original:?} -> {round_trip:?}"
                );
            }
        }
    }
}

#[test]
fn transparent_is_the_identity_for_src_over() {
    let dst = Color::from_srgb8(20, 40, 60, 255);
    assert_eq!(src_over(Color::TRANSPARENT, dst), dst);
}

#[test]
fn an_opaque_source_replaces_the_destination() {
    let src = Color::from_srgb8(255, 0, 0, 255);
    let dst = Color::from_srgb8(0, 0, 255, 255);
    let out = src_over(src, dst);
    assert_eq!(out.to_srgb8(), [255, 0, 0, 255]);
}

#[test]
fn half_over_half_composites_the_premultiplied_way() {
    // 50% white over 50% black: alpha 0.75, and the result is lighter than
    // either. The numbers are the point — this is the operation, not a
    // plausible-looking approximation of it.
    let src = src_over_premul([0.5, 0.5, 0.5, 0.5], [0.0, 0.0, 0.0, 0.5]);
    assert_eq!(src, [0.5, 0.5, 0.5, 0.75]);
}

#[test]
fn coverage_scales_all_four_channels() {
    assert_eq!(
        apply_coverage([0.4, 0.4, 0.4, 0.8], 0.5),
        [0.2, 0.2, 0.2, 0.4]
    );
    assert_eq!(apply_coverage([0.4, 0.4, 0.4, 0.8], 0.0), [0.0; 4]);
}

#[test]
fn colour_space_is_carried_on_the_colour_not_globally() {
    let srgb = Color::from_srgb8(255, 0, 0, 255);
    assert_eq!(srgb.space, ColorSpace::Srgb);
    let p3 = srgb.convert_to(ColorSpace::DisplayP3);
    assert_eq!(p3.space, ColorSpace::DisplayP3);
    // sRGB red sits inside P3's larger gamut, so it needs less than full P3
    // red plus a little green and blue. The reference value is ~0.82.
    let [r, g, b, _] = p3.to_straight();
    assert!((0.80..0.85).contains(&r), "P3 red component was {r}");
    assert!(
        (0.0..0.1).contains(&g) && (0.0..0.1).contains(&b),
        "P3 gave {g}, {b}"
    );
}

#[test]
fn a_wide_gamut_colour_converted_to_srgb_goes_out_of_gamut() {
    // The whole point of keeping components unclamped: P3's reddest red does
    // not exist in sRGB, and saying so is more useful than pretending.
    let p3_red = Color::from_rgba_f32_in(1.0, 0.0, 0.0, 1.0, ColorSpace::DisplayP3);
    let [r, g, b, _] = p3_red.convert_to(ColorSpace::Srgb).to_straight();
    assert!(r > 1.0, "expected an out-of-gamut red, got {r}");
    assert!(
        g < 0.0 || b < 0.0,
        "expected a negative component, got {g}, {b}"
    );
    // But writing it to 8-bit sRGB clamps, because 8-bit sRGB cannot hold it.
    assert_eq!(p3_red.to_srgb8(), [255, 0, 0, 255]);
}

#[test]
fn converting_to_the_same_space_is_a_no_op() {
    let c = Color::from_srgb8(10, 20, 30, 40);
    assert_eq!(c.convert_to(ColorSpace::Srgb), c);
}

#[test]
fn every_colour_space_matrix_is_invertible_and_maps_white_to_d65() {
    // D65 in CIE XYZ, normalised to Y = 1.
    const D65: [f32; 3] = [0.950_47, 1.0, 1.088_83];
    for space in [ColorSpace::Srgb, ColorSpace::DisplayP3, ColorSpace::Rec2020] {
        let to = space.to_xyz();
        assert!(invert3(to).is_some(), "{space:?} matrix is singular");

        let white = apply3(to, [1.0, 1.0, 1.0]);
        for i in 0..3 {
            assert!(
                (white[i] - D65[i]).abs() < 2e-3,
                "{space:?} white point component {i}: {} vs {}",
                white[i],
                D65[i]
            );
        }

        // from_xyz must be the actual inverse, not a separate transcription.
        let round_trip = apply3(space.from_xyz(), white);
        for c in round_trip {
            assert!(
                (c - 1.0).abs() < 1e-4,
                "{space:?} round trip gave {round_trip:?}"
            );
        }
    }
}

#[test]
fn a_singular_matrix_has_no_inverse() {
    assert_eq!(invert3([1.0, 2.0, 3.0, 2.0, 4.0, 6.0, 1.0, 1.0, 1.0]), None);
    assert_eq!(invert3([0.0; 9]), None);
}

#[test]
fn validity_rejects_impossible_premultiplied_colours() {
    // A channel brighter than its own alpha cannot exist premultiplied.
    let bad = Color::from_premul_f32(1.0, 0.0, 0.0, 0.5, ColorSpace::Srgb);
    assert!(!bad.is_valid_premul());
    // As do alpha outside [0, 1] and non-finite components.
    assert!(!Color::from_premul_f32(0.0, 0.0, 0.0, 1.5, ColorSpace::Srgb).is_valid_premul());
    assert!(!Color::from_premul_f32(f32::NAN, 0.0, 0.0, 1.0, ColorSpace::Srgb).is_valid_premul());
    assert!(!Color::from_premul_f32(0.0, -0.1, 0.0, 1.0, ColorSpace::Srgb).is_valid_premul());
    assert!(Color::WHITE.is_valid_premul());
    assert!(Color::TRANSPARENT.is_valid_premul());
}

#[test]
fn interpolation_happens_in_premultiplied_space() {
    // Fading white out to transparent must not darken on the way, which is
    // what interpolating straight components would do.
    let from = Color::from_srgb8(255, 255, 255, 255);
    let to = Color::TRANSPARENT;
    let mid = from.lerp(to, 0.5);
    assert!((mid.a - 0.5).abs() < 1e-6);
    let [r, g, b, _] = mid.to_straight();
    assert!(
        (r - 1.0).abs() < 1e-6 && (g - 1.0).abs() < 1e-6 && (b - 1.0).abs() < 1e-6,
        "white faded to {mid:?}"
    );
}

#[test]
fn opacity_predicates_are_exact_at_the_endpoints() {
    assert!(Color::WHITE.is_opaque());
    assert!(!Color::WHITE.is_transparent());
    assert!(Color::TRANSPARENT.is_transparent());
    assert!(!Color::TRANSPARENT.is_opaque());
    assert!(!Color::from_rgba_f32(1.0, 1.0, 1.0, 0.999).is_opaque());
}

#[test]
fn the_default_colour_is_the_identity_for_src_over() {
    assert_eq!(Color::default(), Color::TRANSPARENT);
    assert_eq!(BlendMode::default(), BlendMode::SrcOver);
    assert_eq!(ColorSpace::default(), ColorSpace::Srgb);
}
