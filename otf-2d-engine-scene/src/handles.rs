//! Handles into the scene arena.
//!
//! Invariant I-2: every reference inside the IR is a `u32` index. No `Rc`, no
//! `Box`, no lifetimes. That is what makes `Scene: Send + Sync`, serialisable
//! without fixups, and cheap to hash.

/// The index a handle uses to mean "nothing". Equal to `u32::MAX`, so it can
/// never collide with a real index: an arena that large would exhaust memory
/// long before reaching it.
pub const NO_REF: u32 = u32::MAX;

macro_rules! handle {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        pub struct $name(pub u32);

        impl $name {
            /// The sentinel meaning "no value".
            pub const NONE: $name = $name(NO_REF);

            #[inline]
            pub const fn new(index: u32) -> Self {
                Self(index)
            }

            #[inline]
            pub const fn index(self) -> u32 {
                self.0
            }

            #[inline]
            pub const fn is_none(self) -> bool {
                self.0 == NO_REF
            }

            /// The index as a `usize`, or `None` for the sentinel.
            #[inline]
            pub const fn get(self) -> Option<usize> {
                if self.0 == NO_REF { None } else { Some(self.0 as usize) }
            }
        }

        impl From<u32> for $name {
            #[inline]
            fn from(index: u32) -> Self {
                Self(index)
            }
        }
    };
}

handle! {
    /// A path in the scene's path arena.
    PathRef
}
handle! {
    /// A paint in the scene's paint arena.
    PaintRef
}
handle! {
    /// An affine in the scene's transform arena.
    TransformRef
}
handle! {
    /// A glyph run in the scene's glyph-run arena.
    GlyphRunRef
}
handle! {
    /// A run of gradient stops in the scene's stop arena.
    StopsRef
}
handle! {
    /// A stroke style in the scene's stroke arena.
    StrokeRef
}
handle! {
    /// A run of variable-font axis coordinates in the scene's arena.
    VariationsRef
}
handle! {
    /// A layer record in the scene's layer arena.
    LayerRef
}
handle! {
    /// An image registered by the caller.
    ///
    /// 2D-Engine never opens a file (Doc 01 §1), so pixel data lives in a
    /// caller-owned registry and the scene stores only this handle.
    ImageRef
}
handle! {
    /// A font registered by the caller.
    ///
    /// As with [`ImageRef`], the font data is the caller's; the scene carries
    /// a handle so it stays `Send + Sync` and serialisable.
    FontRef
}

/// A caller-supplied stable identity for a reusable subtree (Doc 03 §3).
///
/// Typically a DOM node id, widget id or layout box id. Reserved: present in
/// the IR from T1.3, consumed by the node cache in M6. Do not remove it as
/// dead code — the arena layout cannot be changed after the M1 gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct NodeId(pub u64);

/// The hash of a node's encoded content, including its descendants.
///
/// Transform-independent by design (Doc 03 §3): a subtree that only moved is a
/// cache hit with a different transform, which is the common case during
/// scroll. Reserved alongside [`NodeId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct NodeHash(pub u64);

impl NodeHash {
    /// The hash of a node whose content has not been hashed yet.
    pub const UNSET: NodeHash = NodeHash(0);
}
