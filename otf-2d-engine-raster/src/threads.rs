//! The caller-supplied worker pool.
//!
//! 2D-Engine **does not spawn threads** (I-4, D-16). A consumer that already
//! manages its own scheduling should not find a second thread pool appearing
//! underneath it, and wasm may have no threads at all. So the engine describes
//! the work and the caller decides who runs it.
//!
//! # Why the trait hands over byte chunks
//!
//! The obvious shape — "run this closure for indices `0..n`" — cannot give a
//! worker exclusive access to part of the target without either an allocation
//! per frame or unsafe aliasing. Handing the pool the buffer and a chunk size
//! lets it do the split itself with `chunks_mut`, which is safe, allocates
//! nothing, and is exactly what a `rayon` adapter already wants:
//!
//! ```ignore
//! impl ThreadPool for RayonPool {
//!     fn dispatch_chunks(&self, data: &mut [u8], chunk: usize, task: &Task) {
//!         self.pool.install(|| {
//!             data.par_chunks_mut(chunk)
//!                 .enumerate()
//!                 .for_each(|(index, slice)| task(index, slice));
//!         });
//!     }
//! }
//! ```
//!
//! # Why chunks are bands
//!
//! Stage 6 splits the target into bands of whole scanlines, and a band is
//! written by exactly one worker. Nothing is shared, so there is no locking, no
//! false sharing beyond a cache line at the band boundary, and — the part that
//! matters most — the output cannot depend on how the work was scheduled. Bit
//! equality across thread counts is structural, not a property to be tested
//! into existence.

/// Work handed to a pool: `task(index, chunk)` for each chunk in order of
/// index, in any order of execution.
pub type ChunkTask<'a> = dyn Fn(usize, &mut [u8]) + Sync + Send + 'a;

/// A caller-supplied worker pool.
pub trait ThreadPool: Sync {
    /// Splits `data` into consecutive chunks of `chunk` bytes — the last may
    /// be shorter — and runs `task(index, chunk)` for each.
    ///
    /// Must not return until every chunk has been processed. Implementations
    /// are free to run them in any order, on any threads, or serially.
    fn dispatch_chunks(&self, data: &mut [u8], chunk: usize, task: &ChunkTask<'_>);
}

/// Runs the chunks on the calling thread. The behaviour `threads: None`
/// selects, and the reference every pool must match.
#[derive(Debug, Clone, Copy, Default)]
pub struct SerialPool;

impl ThreadPool for SerialPool {
    fn dispatch_chunks(&self, data: &mut [u8], chunk: usize, task: &ChunkTask<'_>) {
        if chunk == 0 {
            return;
        }
        for (index, slice) in data.chunks_mut(chunk).enumerate() {
            task(index, slice);
        }
    }
}
