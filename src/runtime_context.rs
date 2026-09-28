#[cfg(feature = "server")]
use std::cell::RefCell;
#[cfg(feature = "server")]
use std::fmt;
#[cfg(feature = "server")]
use std::future::Future;
#[cfg(feature = "server")]
use std::path::{Path, PathBuf};
#[cfg(feature = "server")]
use std::sync::Arc;
#[cfg(feature = "server")]
use std::sync::OnceLock;
#[cfg(feature = "server")]
use tokio::sync::watch;
#[cfg(feature = "server")]
use ulid::Ulid;
#[cfg(feature = "server")]
tokio::task_local! {
    static REQUEST_RUNTIME_CONTEXT: Arc<RuntimeContext>;
}

#[cfg(feature = "server")]
thread_local! {
    static DEFAULT_RUNTIME_CONTEXT: RefCell<Option<Arc<RuntimeContext>>> = const { RefCell::new(None) };
}

#[cfg(feature = "server")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeId(Ulid);

#[cfg(feature = "server")]
impl RuntimeId {
    #[cfg(any(feature = "dev-config", test))]
    fn new() -> Self {
        Self(Ulid::new())
    }

    #[cfg(test)]
    fn new_test() -> Self {
        Self::new()
    }
}

#[cfg(feature = "server")]
impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(feature = "server")]
#[derive(Debug)]
pub(crate) struct RuntimeContext {
    runtime_id: RuntimeId,
    project_dir: PathBuf,
}

#[cfg(feature = "server")]
impl RuntimeContext {
    #[cfg(any(feature = "dev-config", test))]
    pub(crate) fn new(project_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            runtime_id: RuntimeId::new(),
            project_dir,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_test(project_dir: PathBuf) -> Arc<Self> {
        let _ = RuntimeId::new_test();
        Self::new(project_dir)
    }

    pub(crate) fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }

    pub(crate) fn project_dir(&self) -> &Path {
        &self.project_dir
    }
}

#[cfg(feature = "server")]
pub(crate) fn current_runtime_context() -> Option<Arc<RuntimeContext>> {
    if let Ok(context) = REQUEST_RUNTIME_CONTEXT.try_with(Arc::clone) {
        return Some(context);
    }

    if let Some(server_context) = dioxus::fullstack::FullstackContext::current()
        && let Some(context) = server_context.extension::<Arc<RuntimeContext>>()
    {
        return Some(context);
    }

    if let Some(context) = DEFAULT_RUNTIME_CONTEXT.with(|cell| cell.borrow().clone()) {
        return Some(context);
    }

    None
}

#[cfg(all(feature = "server", any(feature = "dev-config", test)))]
pub(crate) struct DefaultRuntimeContextGuard {
    previous: Option<Arc<RuntimeContext>>,
}

#[cfg(all(feature = "server", any(feature = "dev-config", test)))]
impl Drop for DefaultRuntimeContextGuard {
    fn drop(&mut self) {
        DEFAULT_RUNTIME_CONTEXT.with(|cell| {
            *cell.borrow_mut() = self.previous.take();
        });
    }
}

#[cfg(all(feature = "server", any(feature = "dev-config", test)))]
pub(crate) fn push_default_runtime_context(
    context: Arc<RuntimeContext>,
) -> DefaultRuntimeContextGuard {
    let previous = DEFAULT_RUNTIME_CONTEXT.with(|cell| {
        let mut cell = cell.borrow_mut();
        (*cell).replace(context)
    });

    DefaultRuntimeContextGuard { previous }
}

#[cfg(all(feature = "server", any(feature = "dev-config", test)))]
pub(crate) async fn run_with_runtime_context(
    runtime_context: Arc<RuntimeContext>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let _default_context_guard = push_default_runtime_context(Arc::clone(&runtime_context));
    REQUEST_RUNTIME_CONTEXT
        .scope(runtime_context, next.run(request))
        .await
}

#[cfg(feature = "server")]
pub(crate) fn spawn_with_current_runtime_context<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let runtime_context = current_runtime_context();
    let count = background_task_count().clone();
    count.send_modify(|active| *active += 1);
    let guard = BackgroundTaskGuard(count);
    tokio::spawn(async move {
        let _guard = guard;
        if let Some(runtime_context) = runtime_context {
            REQUEST_RUNTIME_CONTEXT.scope(runtime_context, future).await
        } else {
            future.await
        }
    })
}

#[cfg(feature = "server")]
static BACKGROUND_TASK_COUNT: OnceLock<watch::Sender<usize>> = OnceLock::new();

#[cfg(feature = "server")]
fn background_task_count() -> &'static watch::Sender<usize> {
    BACKGROUND_TASK_COUNT.get_or_init(|| watch::channel(0).0)
}

#[cfg(feature = "server")]
struct BackgroundTaskGuard(watch::Sender<usize>);

#[cfg(feature = "server")]
impl Drop for BackgroundTaskGuard {
    fn drop(&mut self) {
        self.0.send_modify(|active| *active -= 1);
    }
}

#[cfg(feature = "server")]
async fn wait_for_count_zero(mut count: watch::Receiver<usize>) {
    loop {
        if *count.borrow_and_update() == 0 || count.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(feature = "server")]
pub(crate) async fn wait_for_background_tasks() {
    wait_for_count_zero(background_task_count().subscribe()).await;
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
#[tokio::test]
async fn background_task_drain_waits_for_tracked_work() {
    let (count_tx, count_rx) = tokio::sync::watch::channel(1);
    let guard = BackgroundTaskGuard(count_tx);
    let waiter = tokio::spawn(wait_for_count_zero(count_rx));
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    drop(guard);
    waiter.await.expect("drain should finish");
}
