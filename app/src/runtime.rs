//! gpui runs its own executor; the wallet daemon client needs tokio. Network calls run on a shared tokio
//! runtime and are awaited from gpui tasks.

use std::{future::Future, sync::OnceLock};

use tokio::runtime::Runtime;

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Runs `future` on the tokio runtime and resolves with its output.
pub async fn io<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(future).await.expect("wallet task panicked")
}
