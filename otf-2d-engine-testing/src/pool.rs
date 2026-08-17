//! A scoped worker pool for tests.
//!
//! The engine never spawns threads (I-4, D-16) — the caller supplies a pool.
//! This is the caller, for tests and benchmarks. It lives in a `publish = false`
//! test-only crate, outside the invariant gate's scope, which is exactly the
//! separation D-16 is describing: thread creation is the consumer's business.
//!
//! It uses `std::thread::scope`, so the borrow checker enforces that no worker
//! outlives the dispatch call. That is the property the `ThreadPool` contract
//! demands ("must not return until every chunk has been processed") and here
//! it is checked rather than promised.

use otf_2d_engine_raster::{ChunkTask, ThreadPool};

/// A pool that spawns `threads` scoped workers per dispatch.
///
/// Spawning per dispatch rather than keeping workers parked is deliberate:
/// this is a test fixture, and a fixture that owns long-lived threads is a
/// fixture that can hang a test run.
#[derive(Debug, Clone, Copy)]
pub struct ScopedPool {
    threads: usize,
}

impl ScopedPool {
    /// A pool with the given worker count. Zero means one.
    pub fn new(threads: usize) -> ScopedPool {
        ScopedPool {
            threads: threads.max(1),
        }
    }

    pub fn threads(&self) -> usize {
        self.threads
    }
}

impl ThreadPool for ScopedPool {
    fn dispatch_chunks(&self, data: &mut [u8], chunk: usize, task: &ChunkTask<'_>) {
        if chunk == 0 {
            return;
        }
        if self.threads == 1 {
            for (index, slice) in data.chunks_mut(chunk).enumerate() {
                task(index, slice);
            }
            return;
        }

        // Deal the chunks out round-robin so every worker gets a contiguous
        // stride of the buffer rather than one contiguous half: a scene's work
        // is rarely spread evenly down the surface.
        let mut chunks: Vec<(usize, &mut [u8])> = data.chunks_mut(chunk).enumerate().collect();
        let mut lanes: Vec<Vec<(usize, &mut [u8])>> =
            (0..self.threads).map(|_| Vec::new()).collect();
        for (lane, item) in chunks.drain(..).enumerate() {
            let slot = lane % self.threads;
            lanes[slot].push(item);
        }

        std::thread::scope(|scope| {
            for lane in lanes {
                scope.spawn(move || {
                    for (index, slice) in lane {
                        task(index, slice);
                    }
                });
            }
        });
    }
}
