//! ONNX Runtime sessions for CLIP and PaddleOCR.

use ort::environment::ThreadManager;
use ort::session::Session;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use std::path::Path;

/// Worker threads that start at background priority, like the indexer thread that drives them:
/// they use idle cores at full speed and step aside as soon as the user needs the CPU.
struct BackgroundThreads;

impl ThreadManager for BackgroundThreads {
    type Thread = std::thread::JoinHandle<()>;

    fn create(&self, work: impl FnOnce() + Send + 'static) -> ort::Result<Self::Thread> {
        Ok(std::thread::spawn(move || {
            crate::platform::background_thread();
            work();
        }))
    }

    fn join(thread: Self::Thread) -> ort::Result<()> {
        let _ = thread.join();
        Ok(())
    }
}

/// A session whose idle threads sleep. `background`: indexing work on half the cores (at most 4)
/// at background priority; otherwise 2 normal threads for queries the user is waiting on.
/// `arena`: keep memory between runs (much faster for many small runs); without it each run frees
/// what it used.
pub fn session(path: &Path, background: bool, arena: bool) -> anyhow::Result<Session> {
    // Builder errors hold the builder (not Send), so keep only their message.
    let e = |e: ort::Error<SessionBuilder>| anyhow::anyhow!(e.to_string());
    let threads = if background {
        std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).clamp(1, 4))
    } else {
        2
    };
    let mut b = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(e)?
        .with_intra_threads(threads)
        .map_err(e)?
        .with_inter_threads(1)
        .map_err(e)?
        .with_intra_op_spinning(false)
        .map_err(e)?
        .with_memory_pattern(false)
        .map_err(e)?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(arena).build()])
        .map_err(e)?;
    if background {
        b = b.with_thread_manager(BackgroundThreads).map_err(e)?;
    }
    Ok(b.commit_from_file(path)?)
}
