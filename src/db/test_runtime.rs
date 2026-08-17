use super::{
    DbError, close_user_dbs_for_current_runtime, enable_test_mode, enable_user_test_mode,
    reset_test_db,
};
#[cfg(feature = "server")]
use std::path::PathBuf;
#[cfg(feature = "server")]
use std::sync::Arc;
#[cfg(feature = "server")]
use ulid::Ulid;

pub(crate) struct TestRuntimeGuard {
    #[cfg(feature = "server")]
    project_dir: PathBuf,
    #[cfg(feature = "server")]
    runtime_context: Arc<crate::runtime_context::RuntimeContext>,
    #[cfg(feature = "server")]
    runtime_context_guard: Option<crate::runtime_context::DefaultRuntimeContextGuard>,
}

#[cfg(feature = "server")]
impl TestRuntimeGuard {
    pub(crate) fn runtime_context(&self) -> Arc<crate::runtime_context::RuntimeContext> {
        Arc::clone(&self.runtime_context)
    }
}

impl Drop for TestRuntimeGuard {
    fn drop(&mut self) {
        crate::desktop_session::clear();

        #[cfg(feature = "server")]
        {
            reset_test_db();
            let _ = close_user_dbs_for_current_runtime();
            let _ = self.runtime_context_guard.take();
            let _ = std::fs::remove_dir_all(&self.project_dir);
        }
    }
}

pub(crate) fn acquire_test_runtime() -> Result<TestRuntimeGuard, DbError> {
    crate::desktop_session::clear();
    enable_test_mode();
    reset_test_db();
    enable_user_test_mode();

    #[cfg(feature = "server")]
    {
        let project_dir = std::env::temp_dir().join(format!("bitgarth_test_{}", Ulid::new()));
        std::fs::create_dir_all(&project_dir)
            .map_err(|e| DbError::new(format!("Failed to create test project dir: {e}")))?;
        let runtime_context = crate::runtime_context::RuntimeContext::new_test(project_dir.clone());
        let runtime_context_guard =
            crate::runtime_context::push_default_runtime_context(Arc::clone(&runtime_context));

        Ok(TestRuntimeGuard {
            project_dir,
            runtime_context,
            runtime_context_guard: Some(runtime_context_guard),
        })
    }

    #[cfg(not(feature = "server"))]
    {
        Ok(TestRuntimeGuard {})
    }
}
