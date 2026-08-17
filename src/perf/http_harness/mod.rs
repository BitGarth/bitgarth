mod datasets;
mod execution;
mod http_client;
mod measurement;
mod scenarios;
mod server;

use std::fs::create_dir_all;
use std::path::PathBuf;
use std::sync::Arc;

use super::{
    BudgetResult, DatasetId, DatasetShape, PerfDatasetMetadata, PerfError, PerfGitMetadata,
    PerfResultRecord, PerfRunId, PerfRunRequest, ScenarioName, current_build_profile,
    current_git_metadata,
};
use crate::project_paths::{PROJECT_DIR_OVERRIDE_ENV, push_project_dir_override};

pub(super) fn run_http_scenario(request: &PerfRunRequest) -> Result<PerfResultRecord, PerfError> {
    let scenario = scenarios::resolve_scenario(request.scenario_id.as_str())?;
    let run_id = PerfRunId::new();
    let project_dir = resolve_project_dir(request)?;
    create_dir_all(&project_dir).map_err(|err| {
        PerfError::io(format!(
            "failed to create perf project dir {}: {err}",
            project_dir.display()
        ))
    })?;
    let _project_dir_guard = push_project_dir_override(project_dir.clone())
        .map_err(|err| PerfError::io(err.to_string()))?;
    let runtime_context = crate::runtime_context::RuntimeContext::new(project_dir.clone());
    let _runtime_context_guard =
        crate::runtime_context::push_default_runtime_context(Arc::clone(&runtime_context));
    let runtime = match request.mode {
        super::PerfMode::InProcess => Some(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|err| PerfError::io(format!("failed to build perf runtime: {err}")))?,
        ),
        super::PerfMode::RealServer => None,
    };
    let server = match &runtime {
        Some(runtime) => {
            Some(runtime.block_on(server::InProcessServer::start(Arc::clone(&runtime_context)))?)
        }
        None => None,
    };
    let base_url = match &server {
        Some(server) => server.base_url.clone(),
        None => request
            .base_url
            .clone()
            .ok_or_else(|| PerfError::usage("real-server mode requires --base-url"))?,
    };

    let manifest = if request.reuse_dataset.is_some() {
        datasets::read_dataset_manifest(&project_dir)?
    } else {
        datasets::create_dataset_manifest(request, &scenario, &base_url)?
    };

    let session = http_client::login_session(&base_url, &manifest.username, &manifest.password)?;
    let execution = execution::execute_scenario(
        &session,
        &scenario,
        &manifest,
        request.concurrency.unwrap_or(scenario.default_concurrency),
        request
            .warmup_iterations
            .unwrap_or(scenario.default_warmup_iterations),
        request
            .measured_iterations
            .unwrap_or(scenario.default_measured_iterations),
    )?;
    let summary = measurement::summarize_outcomes(&execution.outcomes);
    let budget_result = measurement::evaluate_budget(summary, scenario.budget);
    let git_metadata: PerfGitMetadata = current_git_metadata();
    let dataset_id = DatasetId::parse(manifest.dataset_id.clone())?;
    let dataset_shape = DatasetShape(manifest.dataset_shape.clone());

    Ok(PerfResultRecord {
        run_id,
        timestamp_utc: request.timestamp_utc.clone(),
        scenario_id: request.scenario_id.clone(),
        scenario_name: ScenarioName(scenario.name.to_string()),
        mode: request.mode,
        dataset_id: dataset_id.clone(),
        dataset_shape: dataset_shape.clone(),
        build_profile: current_build_profile(),
        git_commit: git_metadata.git_commit,
        git_dirty: git_metadata.git_dirty,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        endpoint_or_flow: scenario.endpoint_or_flow.to_string(),
        concurrency: request.concurrency.unwrap_or(scenario.default_concurrency),
        warmup_iterations: request
            .warmup_iterations
            .unwrap_or(scenario.default_warmup_iterations),
        measured_iterations: request
            .measured_iterations
            .unwrap_or(scenario.default_measured_iterations),
        median_ms: summary.median_ms,
        p95_ms: summary.p95_ms,
        max_ms: summary.max_ms,
        min_ms: summary.min_ms,
        error_count: summary.error_count,
        success_count: summary.success_count,
        budget_median_ms: scenario.budget.median_ms,
        budget_p95_ms: scenario.budget.p95_ms,
        budget_max_ms: scenario.budget.max_ms,
        budget_result,
        notes: build_result_notes(&project_dir, &execution.notes),
        dataset_metadata: PerfDatasetMetadata {
            dataset_id,
            dataset_shape,
            generated: request.reuse_dataset.is_none(),
            reused_from: request
                .reuse_dataset
                .as_ref()
                .map(|path| path.display().to_string()),
            rough_row_count_marker: manifest.rough_row_count_marker,
            account_count: manifest.account_count,
            address_count: manifest.address_count,
            sync_active: manifest.sync_active,
        },
    })
}

fn resolve_project_dir(request: &PerfRunRequest) -> Result<PathBuf, PerfError> {
    if let Some(path) = &request.reuse_dataset {
        return Ok(path.clone());
    }

    match request.mode {
        super::PerfMode::InProcess => Ok(request.output_dir.join("data")),
        super::PerfMode::RealServer => {
            if std::env::var_os(PROJECT_DIR_OVERRIDE_ENV).is_none() {
                return Err(PerfError::usage(format!(
                    "real-server mode requires {PROJECT_DIR_OVERRIDE_ENV} to be set by the wrapper"
                )));
            }
            crate::project_paths::get_project_dir().map_err(|err| PerfError::io(err.to_string()))
        }
    }
}

fn build_result_notes(project_dir: &std::path::Path, scenario_notes: &[String]) -> String {
    let mut notes = vec![format!("project_dir={}", project_dir.display())];
    notes.extend(scenario_notes.iter().cloned());
    notes.join(" ")
}
