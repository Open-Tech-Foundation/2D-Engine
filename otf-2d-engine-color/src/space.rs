//! Colour spaces.
//!
//! Doc 01 §7: colour space is data carried on the colour, not a global
//! context. Conversion goes through CIE XYZ with a D65 white point, so adding
//! a space means adding one matrix rather than N² conversions.

/// A set of primaries and a white point.
///
/// The transfer function is *not* part of this: every `Color` is linear-light
/// (Doc 01 §7), so encoding curves apply only at the edges of the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorSpace {
    /// sRGB / Rec.709 primaries, D65. The default because it is what
    /// `from_srgb8` produces and what most content is authored in.
    #[default]
    Srgb,
    /// Display P3 primaries, D65. Wider green and red than sRGB.
    DisplayP3,
    /// Rec.2020 primaries, D65. The UHD/HDR container gamut.
    Rec2020,
}

impl ColorSpace {
    /// Row-major 3×3 matrix taking linear RGB in this space to CIE XYZ (D65).
    pub const fn to_xyz(self) -> [f32; 9] {
        match self {
            ColorSpace::Srgb => [
                0.412_390_8,
                0.357_584_3,
                0.180_480_8, //
                0.212_639,
                0.715_168_7,
                0.072_192_3, //
                0.019_330_8,
                0.119_194_8,
                0.950_532_2,
            ],
            ColorSpace::DisplayP3 => [
                0.486_570_9,
                0.265_667_7,
                0.198_217_3, //
                0.228_974_6,
                0.691_738_5,
                0.079_286_9, //
                0.0,
                0.045_113_4,
                1.043_944_4,
            ],
            ColorSpace::Rec2020 => [
                0.636_958,
                0.144_616_9,
                0.168_881, //
                0.262_700_2,
                0.677_998_1,
                0.059_301_7, //
                0.0,
                0.028_072_7,
                1.060_985_1,
            ],
        }
    }

    /// Row-major 3×3 matrix taking CIE XYZ (D65) to linear RGB in this space.
    ///
    /// Inverted from [`ColorSpace::to_xyz`] rather than transcribed, so the
    /// two can never drift apart.
    pub fn from_xyz(self) -> [f32; 9] {
        invert3(self.to_xyz()).expect("a colour space matrix is invertible by construction")
    }

    /// Converts linear RGB components from `self` into `target`.
    ///
    /// Out-of-gamut results are returned as-is, negative components included.
    /// Clamping is a rendering decision made where the pixels are written, not
    /// a conversion decision made here.
    pub fn convert(self, rgb: [f32; 3], target: ColorSpace) -> [f32; 3] {
        if self == target {
            return rgb;
        }
        apply3(target.from_xyz(), apply3(self.to_xyz(), rgb))
    }
}

/// Multiplies a row-major 3×3 matrix by a column vector.
#[inline]
pub fn apply3(m: [f32; 9], v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[1] * v[1] + m[2] * v[2],
        m[3] * v[0] + m[4] * v[1] + m[5] * v[2],
        m[6] * v[0] + m[7] * v[1] + m[8] * v[2],
    ]
}

/// Inverts a row-major 3×3 matrix, or returns `None` when it is singular.
pub fn invert3(m: [f32; 9]) -> Option<[f32; 9]> {
    // Cofactors of the first row give both the determinant and the first
    // column of the adjugate.
    let c00 = m[4] * m[8] - m[5] * m[7];
    let c01 = m[5] * m[6] - m[3] * m[8];
    let c02 = m[3] * m[7] - m[4] * m[6];
    let det = m[0] * c00 + m[1] * c01 + m[2] * c02;
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        c00 * inv,
        (m[2] * m[7] - m[1] * m[8]) * inv,
        (m[1] * m[5] - m[2] * m[4]) * inv,
        c01 * inv,
        (m[0] * m[8] - m[2] * m[6]) * inv,
        (m[2] * m[3] - m[0] * m[5]) * inv,
        c02 * inv,
        (m[1] * m[6] - m[0] * m[7]) * inv,
        (m[0] * m[4] - m[1] * m[3]) * inv,
    ])
}
