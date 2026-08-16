//! The colour type.

use crate::space::ColorSpace;
use crate::srgb;

/// A colour: linear-light, premultiplied, `f32` per channel, carrying its own
/// colour space.
///
/// Doc 01 §7 makes this the model rather than a conversion target. Premultiplied
/// because that is the form compositing wants; storing straight alpha and
/// multiplying in the fine loop costs a multiply per pixel per channel, and
/// storing both invites the two to disagree.
///
/// Components may exceed `[0, 1]`: a Display P3 colour converted to sRGB is
/// legitimately out of gamut, and clamping here would destroy information that
/// a wide-gamut target can show. Clamping happens where pixels are written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    /// Red, premultiplied by `a`.
    pub r: f32,
    /// Green, premultiplied by `a`.
    pub g: f32,
    /// Blue, premultiplied by `a`.
    pub b: f32,
    /// Alpha in `[0, 1]`.
    pub a: f32,
    pub space: ColorSpace,
}

impl Default for Color {
    /// Fully transparent, which is the identity for `src-over`.
    fn default() -> Self {
        Color::TRANSPARENT
    }
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
        space: ColorSpace::Srgb,
    };
    pub const BLACK: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
        space: ColorSpace::Srgb,
    };
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
        space: ColorSpace::Srgb,
    };

    /// From straight-alpha, premultiplied components already in linear light.
    ///
    /// The caller asserts the components are premultiplied; nothing is scaled.
    #[inline]
    pub const fn from_premul_f32(r: f32, g: f32, b: f32, a: f32, space: ColorSpace) -> Color {
        Color { r, g, b, a, space }
    }

    /// From linear-light components with straight (non-premultiplied) alpha.
    ///
    /// This is the constructor Doc 02 §4 names. Components are multiplied by
    /// `a` on the way in.
    #[inline]
    pub fn from_rgba_f32(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color::from_rgba_f32_in(r, g, b, a, ColorSpace::Srgb)
    }

    /// [`Color::from_rgba_f32`] in a named colour space.
    #[inline]
    pub fn from_rgba_f32_in(r: f32, g: f32, b: f32, a: f32, space: ColorSpace) -> Color {
        Color {
            r: r * a,
            g: g * a,
            b: b * a,
            a,
            space,
        }
    }

    /// From 8-bit sRGB with straight alpha — the common case.
    ///
    /// Decodes the transfer function and premultiplies, so `#1e1e24ff` written
    /// in a stylesheet arrives as the linear colour it denotes.
    #[inline]
    pub fn from_srgb8(r: u8, g: u8, b: u8, a: u8) -> Color {
        let alpha = srgb::alpha8_to_f32(a);
        Color {
            r: srgb::srgb8_to_linear(r) * alpha,
            g: srgb::srgb8_to_linear(g) * alpha,
            b: srgb::srgb8_to_linear(b) * alpha,
            a: alpha,
            space: ColorSpace::Srgb,
        }
    }

    /// From a packed `0xRRGGBBAA` literal.
    #[inline]
    pub fn from_rgba8_hex(hex: u32) -> Color {
        Color::from_srgb8(
            (hex >> 24) as u8,
            (hex >> 16) as u8,
            (hex >> 8) as u8,
            hex as u8,
        )
    }

    /// The linear-light components with alpha divided back out.
    ///
    /// A fully transparent colour has no recoverable hue, so it unpremultiplies
    /// to zeroes rather than to a division by zero.
    #[inline]
    pub fn to_straight(self) -> [f32; 4] {
        if self.a <= 0.0 {
            return [0.0, 0.0, 0.0, 0.0];
        }
        let inv = 1.0 / self.a;
        [self.r * inv, self.g * inv, self.b * inv, self.a]
    }

    /// The premultiplied components, in storage order.
    #[inline]
    pub const fn to_premul(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Encodes back to 8-bit sRGB with straight alpha.
    ///
    /// Converts to sRGB primaries first if the colour is in another space, and
    /// clamps out-of-gamut components — 8-bit sRGB cannot represent them.
    pub fn to_srgb8(self) -> [u8; 4] {
        let [r, g, b, a] = self.to_straight();
        let [r, g, b] = self.space.convert([r, g, b], ColorSpace::Srgb);
        [
            srgb::linear_to_srgb8(r),
            srgb::linear_to_srgb8(g),
            srgb::linear_to_srgb8(b),
            srgb::f32_to_alpha8(a),
        ]
    }

    /// The same colour expressed in `target`.
    ///
    /// Conversion is defined on straight components, so this unpremultiplies,
    /// converts and premultiplies again.
    pub fn convert_to(self, target: ColorSpace) -> Color {
        if self.space == target {
            return self;
        }
        let [r, g, b, a] = self.to_straight();
        let [r, g, b] = self.space.convert([r, g, b], target);
        Color::from_rgba_f32_in(r, g, b, a, target)
    }

    /// This colour with its alpha multiplied by `factor`.
    ///
    /// Premultiplied storage makes group opacity a single scale of all four
    /// channels, which is why `push_layer`'s alpha is cheap.
    #[inline]
    pub fn with_alpha_multiplied(self, factor: f32) -> Color {
        Color {
            r: self.r * factor,
            g: self.g * factor,
            b: self.b * factor,
            a: self.a * factor,
            space: self.space,
        }
    }

    /// True when alpha is 1, so nothing behind this colour can show through.
    #[inline]
    pub fn is_opaque(self) -> bool {
        self.a >= 1.0
    }

    /// True when alpha is 0, so drawing it changes nothing.
    #[inline]
    pub fn is_transparent(self) -> bool {
        self.a <= 0.0
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.r.is_finite() && self.g.is_finite() && self.b.is_finite() && self.a.is_finite()
    }

    /// True when every component lies in the range premultiplied storage
    /// allows: alpha in `[0, 1]` and no channel exceeding it.
    ///
    /// Encode-time validation uses this; a colour that fails it would produce
    /// undefined results in the fine rasterizer.
    #[inline]
    pub fn is_valid_premul(self) -> bool {
        self.is_finite()
            && (0.0..=1.0).contains(&self.a)
            && self.r >= 0.0
            && self.g >= 0.0
            && self.b >= 0.0
            && self.r <= self.a
            && self.g <= self.a
            && self.b <= self.a
    }

    /// Linear interpolation in premultiplied space, which is the correct place
    /// to interpolate: blending straight components across an alpha ramp
    /// produces the classic dark fringe.
    #[inline]
    pub fn lerp(self, other: Color, t: f32) -> Color {
        let other = other.convert_to(self.space);
        Color {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
            space: self.space,
        }
    }
}
