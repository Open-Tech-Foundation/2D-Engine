//! Byte serialisation for [`Scene`].
//!
//! The scene is already a set of flat POD buffers with `u32` handles instead of
//! pointers (I-2), so serialising it is a header plus a `memcpy` per buffer —
//! no traversal, no pointer fixups, no intermediate representation.
//!
//! # This is a cache format, not an interchange format
//!
//! The payload is raw record bytes in host layout and host byte order. That is
//! the point: it makes `to_bytes`/`from_bytes` cheap enough to sit in front of
//! a disk cache. It also means the bytes are only valid for the machine and
//! the build that produced them, so the header carries three guards — an
//! endianness sentinel, a record-layout fingerprint, and a format version —
//! and [`Scene::from_bytes`] refuses anything that does not match.
//!
//! Decoding is total: every input either produces a `Scene` whose handles are
//! all in range, or an error. It never produces a scene that indexes out of
//! bounds downstream.

use alloc::vec::Vec;
use core::fmt;
use core::mem::size_of;

use bytemuck::Pod;

use crate::handles::NO_REF;
use crate::records::{
    ColorStopRec, DrawKind, DrawTag, GlyphRec, GlyphRunDesc, LayerDesc, NodeDesc, PaintDesc,
    PaintKind, PathDesc, RunRec, ShapeKind, StrokeDesc, TransformRec,
};
use crate::scene::Scene;
use crate::unit::SceneUnit;

/// `b"OTF2DSCN"`.
const MAGIC: [u8; 8] = *b"OTF2DSCN";

/// Bumped whenever the header or the buffer order changes. A record *layout*
/// change is caught by [`layout_id`] instead, which needs no discipline to
/// maintain.
const VERSION: u32 = 1;

/// Written in host byte order. A reader on the other endianness sees the
/// byte-reversed value and rejects the input rather than reading garbage.
const ENDIAN_SENTINEL: u32 = 0x0102_0304;

/// Number of buffers, and so the number of counts in the header.
const BUFFER_COUNT: usize = 17;

/// `magic 8 | version 4 | endian 4 | layout 4 | unit 1 | pad 3 | counts 8*N`.
const HEADER_LEN: usize = 24 + BUFFER_COUNT * 8;

/// Every buffer starts on an eight-byte boundary so that a future zero-copy
/// reader can cast in place; `f64` and `u64` records need it.
const ALIGN: usize = 8;

/// A fingerprint of every record's size.
///
/// Adding a field to a record changes this, so stale bytes are rejected even
/// if someone forgets to bump [`VERSION`]. FNV-1a over the sizes, chosen
/// because it is four lines and needs no state.
fn layout_id() -> u32 {
    let sizes = [
        size_of::<DrawTag>(),
        size_of::<PathDesc>(),
        size_of::<TransformRec>(),
        size_of::<PaintDesc>(),
        size_of::<ColorStopRec>(),
        size_of::<StrokeDesc>(),
        size_of::<GlyphRunDesc>(),
        size_of::<GlyphRec>(),
        size_of::<LayerDesc>(),
        size_of::<NodeDesc>(),
        size_of::<RunRec>(),
    ];
    let mut hash: u32 = 0x811c_9dc5;
    for size in sizes {
        hash ^= size as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Why a byte slice is not a scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SceneDecodeError {
    /// The input does not start with `OTF2DSCN`.
    BadMagic,
    /// The input ended before a declared buffer did.
    Truncated {
        /// Bytes the header said were there.
        needed: usize,
        /// Bytes actually supplied.
        found: usize,
    },
    /// Written by a different version of this format.
    UnsupportedVersion {
        /// The version in the header.
        found: u32,
        /// The version this build writes.
        expected: u32,
    },
    /// Written on a machine with the opposite byte order.
    ForeignEndian,
    /// Written by a build whose record layout differs.
    LayoutMismatch {
        /// The fingerprint in the header.
        found: u32,
        /// This build's fingerprint.
        expected: u32,
    },
    /// The unit byte is not a [`SceneUnit`] discriminant.
    UnknownUnit(u8),
    /// A declared element count does not fit in this machine's address space.
    CountOverflow,
    /// A record holds a discriminant that no enum variant claims.
    UnknownDiscriminant {
        /// Which buffer the record is in.
        buffer: &'static str,
        /// The record's index.
        index: usize,
    },
    /// A handle points outside the buffer it indexes.
    DanglingReference {
        /// Which buffer the offending record is in.
        buffer: &'static str,
        /// The record's index.
        index: usize,
    },
}

impl fmt::Display for SceneDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a 2D-Engine scene: bad magic"),
            Self::Truncated { needed, found } => {
                write!(f, "scene is truncated: needs {needed} bytes, got {found}")
            }
            Self::UnsupportedVersion { found, expected } => {
                write!(
                    f,
                    "scene format version {found}, this build writes {expected}"
                )
            }
            Self::ForeignEndian => write!(f, "scene was written with the opposite byte order"),
            Self::LayoutMismatch { found, expected } => {
                write!(
                    f,
                    "scene record layout {found:#010x} != this build's {expected:#010x}"
                )
            }
            Self::UnknownUnit(v) => write!(f, "unknown scene unit {v}"),
            Self::CountOverflow => write!(f, "scene declares more elements than can be addressed"),
            Self::UnknownDiscriminant { buffer, index } => {
                write!(f, "unknown discriminant in {buffer}[{index}]")
            }
            Self::DanglingReference { buffer, index } => {
                write!(f, "handle out of range in {buffer}[{index}]")
            }
        }
    }
}

impl core::error::Error for SceneDecodeError {}

// ---------------------------------------------------------------- writing

/// Rounds `n` up to the next multiple of [`ALIGN`].
#[inline]
const fn aligned(n: usize) -> usize {
    n.next_multiple_of(ALIGN)
}

/// Appends `data` and pads to the next eight-byte boundary *relative to
/// `start`*, not to `out`. Padding relative to the buffer would make the bytes
/// depend on what was already in `out`, and the decoder measures from the
/// start of the message.
fn push_buffer<T: Pod>(out: &mut Vec<u8>, start: usize, data: &[T]) {
    out.extend_from_slice(bytemuck::cast_slice(data));
    out.resize(start + aligned(out.len() - start), 0);
}

impl Scene {
    /// Serialises the arena to bytes.
    ///
    /// Round-tripping preserves [`Scene::content_hash`] exactly: the hash is
    /// taken over the same buffers this writes, in the same order.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.memory_usage().total() + 128);
        self.write_to(&mut out);
        out
    }

    /// Appends the serialised arena to `out`.
    ///
    /// Use this instead of [`Scene::to_bytes`] when writing many scenes into
    /// one reused buffer: it allocates nothing once `out` has the capacity.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_ne_bytes());
        out.extend_from_slice(&ENDIAN_SENTINEL.to_ne_bytes());
        out.extend_from_slice(&layout_id().to_ne_bytes());
        out.push(self.unit.to_u8());
        out.extend_from_slice(&[0u8; 3]);

        let counts: [u64; BUFFER_COUNT] = [
            self.tags.len() as u64,
            self.path_data.len() as u64,
            self.path_verbs.len() as u64,
            self.paths.len() as u64,
            self.transforms.len() as u64,
            self.paints.len() as u64,
            self.stops.len() as u64,
            self.stop_runs.len() as u64,
            self.strokes.len() as u64,
            self.dash_data.len() as u64,
            self.glyph_runs.len() as u64,
            self.glyphs.len() as u64,
            self.variations.len() as u64,
            self.variation_runs.len() as u64,
            self.layers.len() as u64,
            self.node_hashes.len() as u64,
            self.node_descs.len() as u64,
        ];
        for count in counts {
            out.extend_from_slice(&count.to_ne_bytes());
        }
        debug_assert_eq!(out.len() - start, HEADER_LEN);

        push_buffer(out, start, &self.tags);
        push_buffer(out, start, &self.path_data);
        push_buffer(out, start, &self.path_verbs);
        push_buffer(out, start, &self.paths);
        push_buffer(out, start, &self.transforms);
        push_buffer(out, start, &self.paints);
        push_buffer(out, start, &self.stops);
        push_buffer(out, start, &self.stop_runs);
        push_buffer(out, start, &self.strokes);
        push_buffer(out, start, &self.dash_data);
        push_buffer(out, start, &self.glyph_runs);
        push_buffer(out, start, &self.glyphs);
        push_buffer(out, start, &self.variations);
        push_buffer(out, start, &self.variation_runs);
        push_buffer(out, start, &self.layers);
        push_buffer(out, start, &self.node_hashes);
        push_buffer(out, start, &self.node_descs);
    }
}

// ---------------------------------------------------------------- reading

/// A bounds-checked cursor over the payload.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], SceneDecodeError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(SceneDecodeError::CountOverflow)?;
        let padded = aligned(end);
        if padded > self.bytes.len() {
            return Err(SceneDecodeError::Truncated {
                needed: padded,
                found: self.bytes.len(),
            });
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = padded;
        Ok(slice)
    }

    /// Reads `count` records.
    ///
    /// Element-wise rather than a cast because the payload is only guaranteed
    /// eight-byte aligned *within the message*, and the caller's slice may
    /// start anywhere. `pod_read_unaligned` is the safe way to say that.
    fn buffer<T: Pod>(&mut self, count: usize) -> Result<Vec<T>, SceneDecodeError> {
        let size = size_of::<T>();
        let len = count
            .checked_mul(size)
            .ok_or(SceneDecodeError::CountOverflow)?;
        let bytes = self.take(len)?;
        let mut out = Vec::with_capacity(count);
        for chunk in bytes.chunks_exact(size) {
            out.push(bytemuck::pod_read_unaligned::<T>(chunk));
        }
        Ok(out)
    }
}

/// True when `offset .. offset + len` fits inside `total` elements.
#[inline]
fn spans(offset: u32, len: u32, total: usize) -> bool {
    (offset as usize)
        .checked_add(len as usize)
        .is_some_and(|end| end <= total)
}

/// True when a handle is either absent or in range.
#[inline]
fn optional(handle: u32, total: usize) -> bool {
    handle == NO_REF || (handle as usize) < total
}

impl Scene {
    /// Reconstructs a scene from [`Scene::to_bytes`] output.
    ///
    /// Rejects bytes from a different format version, byte order or record
    /// layout, and validates every handle, so a decoded scene is as safe to
    /// hand to stage 2 as an encoded one.
    pub fn from_bytes(bytes: &[u8]) -> Result<Scene, SceneDecodeError> {
        if bytes.len() < HEADER_LEN {
            if bytes.len() < MAGIC.len() || bytes[..MAGIC.len()] != MAGIC {
                return Err(SceneDecodeError::BadMagic);
            }
            return Err(SceneDecodeError::Truncated {
                needed: HEADER_LEN,
                found: bytes.len(),
            });
        }
        if bytes[..8] != MAGIC {
            return Err(SceneDecodeError::BadMagic);
        }
        let word = |at: usize| {
            u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };

        let version = word(8);
        if version != VERSION {
            return Err(SceneDecodeError::UnsupportedVersion {
                found: version,
                expected: VERSION,
            });
        }
        if word(12) != ENDIAN_SENTINEL {
            return Err(SceneDecodeError::ForeignEndian);
        }
        let layout = word(16);
        if layout != layout_id() {
            return Err(SceneDecodeError::LayoutMismatch {
                found: layout,
                expected: layout_id(),
            });
        }
        let unit = SceneUnit::from_u8(bytes[20]).ok_or(SceneDecodeError::UnknownUnit(bytes[20]))?;

        let mut counts = [0usize; BUFFER_COUNT];
        for (i, count) in counts.iter_mut().enumerate() {
            let at = 24 + i * 8;
            let raw = u64::from_ne_bytes(bytes[at..at + 8].try_into().expect("eight bytes"));
            *count = usize::try_from(raw).map_err(|_| SceneDecodeError::CountOverflow)?;
        }

        let mut reader = Reader {
            bytes,
            pos: HEADER_LEN,
        };
        let scene = Scene {
            tags: reader.buffer(counts[0])?,
            path_data: reader.buffer(counts[1])?,
            path_verbs: reader.buffer(counts[2])?,
            paths: reader.buffer(counts[3])?,
            transforms: reader.buffer(counts[4])?,
            paints: reader.buffer(counts[5])?,
            stops: reader.buffer(counts[6])?,
            stop_runs: reader.buffer(counts[7])?,
            strokes: reader.buffer(counts[8])?,
            dash_data: reader.buffer(counts[9])?,
            glyph_runs: reader.buffer(counts[10])?,
            glyphs: reader.buffer(counts[11])?,
            variations: reader.buffer(counts[12])?,
            variation_runs: reader.buffer(counts[13])?,
            layers: reader.buffer(counts[14])?,
            node_hashes: reader.buffer(counts[15])?,
            node_descs: reader.buffer(counts[16])?,
            unit,
        };
        scene.validate()?;
        Ok(scene)
    }

    /// Checks that every handle in the arena is in range.
    ///
    /// The encoder cannot produce a scene that fails this. Decoding untrusted
    /// bytes can, which is the whole reason it exists.
    pub(crate) fn validate(&self) -> Result<(), SceneDecodeError> {
        let dangling = |buffer, index| Err(SceneDecodeError::DanglingReference { buffer, index });
        let unknown = |buffer, index| Err(SceneDecodeError::UnknownDiscriminant { buffer, index });

        for (i, path) in self.paths.iter().enumerate() {
            if !spans(path.verb_offset, path.verb_len, self.path_verbs.len())
                || !spans(path.point_offset, path.point_len, self.path_data.len())
                || path.point_len % 2 != 0
            {
                return dangling("paths", i);
            }
            if ShapeKind::from_u32(path.shape).is_none() {
                return unknown("paths", i);
            }
        }

        for (i, paint) in self.paints.iter().enumerate() {
            if PaintKind::from_u32(paint.kind).is_none() {
                return unknown("paints", i);
            }
            if !spans(paint.stops_offset, paint.stops_len, self.stops.len())
                || !optional(paint.transform, self.transforms.len())
            {
                return dangling("paints", i);
            }
        }

        for (i, run) in self.stop_runs.iter().enumerate() {
            if !spans(run.offset, run.len, self.stops.len()) {
                return dangling("stop_runs", i);
            }
        }

        for (i, run) in self.variation_runs.iter().enumerate() {
            if !spans(run.offset, run.len, self.variations.len()) {
                return dangling("variation_runs", i);
            }
        }

        for (i, stroke) in self.strokes.iter().enumerate() {
            if !spans(
                stroke.dash_offset_index,
                stroke.dash_len,
                self.dash_data.len(),
            ) {
                return dangling("strokes", i);
            }
        }

        for (i, run) in self.glyph_runs.iter().enumerate() {
            if !spans(run.glyph_offset, run.glyph_len, self.glyphs.len())
                || !spans(
                    run.variations_offset,
                    run.variations_len,
                    self.variations.len(),
                )
            {
                return dangling("glyph_runs", i);
            }
        }

        for (i, layer) in self.layers.iter().enumerate() {
            if !optional(layer.clip_path, self.paths.len())
                || !optional(layer.transform, self.transforms.len())
                || !optional(layer.push_tag, self.tags.len())
                || !optional(layer.pop_tag, self.tags.len())
            {
                return dangling("layers", i);
            }
        }

        for (i, tag) in self.tags.iter().enumerate() {
            let Some(kind) = DrawKind::from_u8(tag.kind) else {
                return unknown("tags", i);
            };
            if !optional(tag.transform, self.transforms.len())
                || !optional(tag.paint, self.paints.len())
            {
                return dangling("tags", i);
            }
            let payload_ok = match kind {
                DrawKind::Fill | DrawKind::Stroke => optional(tag.payload, self.paths.len()),
                DrawKind::Glyphs => optional(tag.payload, self.glyph_runs.len()),
                // An image handle indexes the caller's registry, which the
                // scene does not own and cannot bound.
                DrawKind::Image => true,
                DrawKind::PushLayer | DrawKind::PopLayer => {
                    optional(tag.payload, self.layers.len())
                }
            };
            let aux_ok = match kind {
                DrawKind::Stroke => optional(tag.aux, self.strokes.len()),
                _ => true,
            };
            if !payload_ok || !aux_ok {
                return dangling("tags", i);
            }
        }

        if self.node_hashes.len() != self.node_descs.len() {
            return dangling(
                "node_hashes",
                self.node_hashes.len().min(self.node_descs.len()),
            );
        }
        for (i, node) in self.node_descs.iter().enumerate() {
            if !spans(node.tag_offset, node.tag_len, self.tags.len())
                || !optional(node.parent, self.node_descs.len())
            {
                return dangling("node_descs", i);
            }
        }

        Ok(())
    }
}
