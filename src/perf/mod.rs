mod http_harness;

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use std::ffi::OsString;
use std::fmt;
use std::fs::{create_dir_all, write};
use std::path::{Path, PathBuf};
use std::process::Command;
use ulid::Ulid;

const PERF_COMMAND: &str = "perf";
const PERF_RUN_COMMAND: &str = "run";
const DEFAULT_OUTPUT_ROOT: &str = "test-results/perf";
const PLACEHOLDER_SCENARIO_ID: &str = "placeholder";
const PLACEHOLDER_DATASET_ID: &str = "tiny-empty";
const PLACEHOLDER_DATASET_SHAPE: &str = "placeholder";
const SUMMARY_FILENAME: &str = "summary.md";
const RUN_FILENAME: &str = "run.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PerfRunId(String);

impl PerfRunId {
    fn new() -> Self {
        Self(Ulid::new().to_string())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ScenarioId(String);

impl ScenarioId {
    fn placeholder() -> Self {
        Self(PLACEHOLDER_SCENARIO_ID.to_string())
    }

    fn parse(value: String) -> Result<Self, PerfError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(PerfError::usage("scenario id cannot be empty"));
        }

        Ok(Self(trimmed.to_string()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ScenarioName(String);

impl ScenarioName {
    fn placeholder() -> Self {
        Self("Placeholder Scenario".to_string())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DatasetId(String);

impl DatasetId {
    fn placeholder() -> Self {
        Self(PLACEHOLDER_DATASET_ID.to_string())
    }

    fn parse(value: String) -> Result<Self, PerfError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(PerfError::usage("dataset id cannot be empty"));
        }

        Ok(Self(trimmed.to_string()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DatasetShape(String);

impl DatasetShape {
    fn placeholder() -> Self {
        Self(PLACEHOLDER_DATASET_SHAPE.to_string())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PerfMode {
    InProcess,
    RealServer,
}

impl PerfMode {
    fn parse(value: &str) -> Result<Self, PerfError> {
        match value {
            "in-process" | "in_process" => Ok(Self::InProcess),
            "real-server" | "real_server" => Ok(Self::RealServer),
            other => Err(PerfError::usage(format!(
                "unsupported mode '{other}', expected in-process or real-server"
            ))),
        }
    }

    fn as_cli_value(self) -> &'static str {
        match self {
            Self::InProcess => "in-process",
            Self::RealServer => "real-server",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BudgetResult {
    Passed,
    Failed,
    ReportOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PerfDatasetMetadata {
    dataset_id: DatasetId,
    dataset_shape: DatasetShape,
    generated: bool,
    reused_from: Option<String>,
    rough_row_count_marker: Option<u64>,
    account_count: Option<u32>,
    address_count: Option<u32>,
    sync_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PerfGitMetadata {
    git_commit: Option<String>,
    git_dirty: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PerfResultRecord {
    run_id: PerfRunId,
    timestamp_utc: String,
    scenario_id: ScenarioId,
    scenario_name: ScenarioName,
    mode: PerfMode,
    dataset_id: DatasetId,
    dataset_shape: DatasetShape,
    build_profile: String,
    git_commit: Option<String>,
    git_dirty: Option<bool>,
    app_version: String,
    endpoint_or_flow: String,
    concurrency: u32,
    warmup_iterations: u32,
    measured_iterations: u32,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    min_ms: f64,
    error_count: u32,
    success_count: u32,
    budget_median_ms: Option<f64>,
    budget_p95_ms: Option<f64>,
    budget_max_ms: Option<f64>,
    budget_result: BudgetResult,
    notes: String,
    dataset_metadata: PerfDatasetMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PerfRunRequest {
    mode: PerfMode,
    base_url: Option<String>,
    scenario_id: ScenarioId,
    dataset_id: DatasetId,
    timestamp_utc: String,
    output_dir: PathBuf,
    reuse_dataset: Option<PathBuf>,
    warmup_iterations: Option<u32>,
    measured_iterations: Option<u32>,
    concurrency: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PerfCommand {
    Run(PerfRunRequest),
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PerfPaths {
    pub(crate) output_dir: PathBuf,
    pub(crate) run_path: PathBuf,
    pub(crate) summary_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PerfError {
    Usage(String),
    Io(String),
    Json(String),
    HttpClient(String),
    BudgetFailed(String),
}

impl PerfError {
    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    fn io(message: impl Into<String>) -> Self {
        Self::Io(message.into())
    }
}

impl fmt::Display for PerfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::Io(message) => write!(f, "{message}"),
            Self::Json(message) => write!(f, "{message}"),
            Self::HttpClient(message) => write!(f, "{message}"),
            Self::BudgetFailed(message) => write!(f, "{message}"),
        }
    }
}

pub(crate) fn maybe_run_from_args() -> Result<bool, PerfError> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().and_then(|arg| arg.to_str()) != Some(PERF_COMMAND) {
        return Ok(false);
    }

    dioxus::logger::init(tracing::Level::INFO)
        .map_err(|err| PerfError::io(format!("failed to init perf logger: {err}")))?;

    let command = parse_command(&args[1..])?;
    match command {
        PerfCommand::Help => {
            print_usage();
        }
        PerfCommand::Run(request) => {
            let record = if request.scenario_id.as_str() == PLACEHOLDER_SCENARIO_ID {
                run_placeholder_scenario(&request)?
            } else {
                http_harness::run_http_scenario(&request)?
            };
            let paths = persist_result(&request.output_dir, &record)?;
            print_summary(&record, &paths);
            enforce_budget_result(&record)?;
        }
    }

    Ok(true)
}

fn parse_command(args: &[OsString]) -> Result<PerfCommand, PerfError> {
    let Some(subcommand) = args.first().and_then(|value| value.to_str()) else {
        return Ok(PerfCommand::Help);
    };

    match subcommand {
        "help" | "--help" | "-h" => Ok(PerfCommand::Help),
        PERF_RUN_COMMAND => parse_run_command(&args[1..]).map(PerfCommand::Run),
        other => Err(PerfError::usage(format!(
            "unknown perf subcommand '{other}'"
        ))),
    }
}

fn parse_run_command(args: &[OsString]) -> Result<PerfRunRequest, PerfError> {
    let mut mode = PerfMode::InProcess;
    let mut base_url = None;
    let mut scenario_id = ScenarioId::placeholder();
    let mut dataset_id = DatasetId::placeholder();
    let mut timestamp_utc = None;
    let mut output_dir = None;
    let mut reuse_dataset = None;
    let mut warmup_iterations = None;
    let mut measured_iterations = None;
    let mut concurrency = None;

    let mut index = 0;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| PerfError::usage("perf arguments must be valid UTF-8"))?;
        match flag {
            "--mode" => {
                let value = next_flag_value(args, &mut index, "--mode")?;
                mode = PerfMode::parse(value)?;
            }
            "--base-url" => {
                let value = next_flag_value(args, &mut index, "--base-url")?;
                base_url = Some(normalize_base_url(value)?);
            }
            "--scenario" => {
                let value = next_flag_value(args, &mut index, "--scenario")?;
                scenario_id = ScenarioId::parse(value.to_string())?;
            }
            "--dataset" => {
                let value = next_flag_value(args, &mut index, "--dataset")?;
                dataset_id = DatasetId::parse(value.to_string())?;
            }
            "--timestamp" => {
                let value = next_flag_value(args, &mut index, "--timestamp")?;
                timestamp_utc = Some(normalize_timestamp_utc(value)?);
            }
            "--output-dir" => {
                let value = next_flag_value(args, &mut index, "--output-dir")?;
                output_dir = Some(PathBuf::from(value));
            }
            "--reuse-dataset" => {
                let value = next_flag_value(args, &mut index, "--reuse-dataset")?;
                reuse_dataset = Some(PathBuf::from(value));
            }
            "--warmup" => {
                let value = next_flag_value(args, &mut index, "--warmup")?;
                warmup_iterations = Some(parse_u32_flag("--warmup", value)?);
            }
            "--iterations" => {
                let value = next_flag_value(args, &mut index, "--iterations")?;
                measured_iterations = Some(parse_u32_flag("--iterations", value)?);
            }
            "--concurrency" => {
                let value = next_flag_value(args, &mut index, "--concurrency")?;
                concurrency = Some(parse_u32_flag("--concurrency", value)?);
            }
            "--help" | "-h" => return Ok(default_run_request()),
            other => {
                return Err(PerfError::usage(format!("unknown perf run flag '{other}'")));
            }
        }
        index += 1;
    }

    match (mode, base_url.as_ref()) {
        (PerfMode::InProcess, Some(_)) => {
            return Err(PerfError::usage(
                "--base-url is only valid in real-server mode",
            ));
        }
        (PerfMode::RealServer, None) => {
            return Err(PerfError::usage("real-server mode requires --base-url"));
        }
        (PerfMode::InProcess, None) | (PerfMode::RealServer, Some(_)) => {}
    }

    let timestamp_utc = timestamp_utc.unwrap_or_else(current_timestamp_utc);
    let output_dir = output_dir.unwrap_or_else(|| default_output_dir(&timestamp_utc));

    Ok(PerfRunRequest {
        mode,
        base_url,
        scenario_id,
        dataset_id,
        timestamp_utc,
        output_dir,
        reuse_dataset,
        warmup_iterations,
        measured_iterations,
        concurrency,
    })
}

fn default_run_request() -> PerfRunRequest {
    let timestamp_utc = current_timestamp_utc();
    PerfRunRequest {
        mode: PerfMode::InProcess,
        base_url: None,
        scenario_id: ScenarioId::placeholder(),
        dataset_id: DatasetId::placeholder(),
        timestamp_utc: timestamp_utc.clone(),
        output_dir: default_output_dir(&timestamp_utc),
        reuse_dataset: None,
        warmup_iterations: None,
        measured_iterations: None,
        concurrency: None,
    }
}

fn current_timestamp_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn normalize_timestamp_utc(value: &str) -> Result<String, PerfError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|parsed| {
            parsed
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        })
        .map_err(|err| PerfError::usage(format!("invalid value for --timestamp: {err}")))
}

fn normalize_base_url(value: &str) -> Result<String, PerfError> {
    let mut parsed = url::Url::parse(value)
        .map_err(|err| PerfError::usage(format!("invalid --base-url '{value}': {err}")))?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(PerfError::usage(format!(
            "--base-url must use http or https: {value}"
        )));
    }
    if !matches!(parsed.host_str(), Some("127.0.0.1") | Some("localhost")) {
        return Err(PerfError::usage(format!(
            "--base-url must point to 127.0.0.1 or localhost: {value}"
        )));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(PerfError::usage(format!(
            "--base-url must not contain credentials, query, or fragment: {value}"
        )));
    }

    if !parsed.path().ends_with('/') {
        parsed.set_path(&format!("{}/", parsed.path()));
    }

    Ok(parsed.into())
}

fn default_output_dir(timestamp_utc: &str) -> PathBuf {
    PathBuf::from(DEFAULT_OUTPUT_ROOT).join(timestamp_utc.replace(':', "-"))
}

fn parse_u32_flag(flag: &str, value: &str) -> Result<u32, PerfError> {
    value
        .parse::<u32>()
        .map_err(|err| PerfError::usage(format!("invalid value for {flag}: {err}")))
}

fn next_flag_value<'a>(
    args: &'a [OsString],
    index: &mut usize,
    flag: &str,
) -> Result<&'a str, PerfError> {
    let value = args
        .get(*index + 1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| PerfError::usage(format!("missing value for {flag}")))?;
    *index += 1;
    Ok(value)
}

fn run_placeholder_scenario(request: &PerfRunRequest) -> Result<PerfResultRecord, PerfError> {
    let run_id = PerfRunId::new();
    let dataset_shape = DatasetShape::placeholder();
    let notes = placeholder_notes(request);
    let git_metadata = current_git_metadata();

    Ok(PerfResultRecord {
        run_id,
        timestamp_utc: request.timestamp_utc.clone(),
        scenario_id: request.scenario_id.clone(),
        scenario_name: ScenarioName::placeholder(),
        mode: request.mode,
        dataset_id: request.dataset_id.clone(),
        dataset_shape: dataset_shape.clone(),
        build_profile: current_build_profile(),
        git_commit: git_metadata.git_commit,
        git_dirty: git_metadata.git_dirty,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        endpoint_or_flow: "placeholder".to_string(),
        concurrency: 1,
        warmup_iterations: 0,
        measured_iterations: 1,
        median_ms: 0.0,
        p95_ms: 0.0,
        max_ms: 0.0,
        min_ms: 0.0,
        error_count: 0,
        success_count: 1,
        budget_median_ms: None,
        budget_p95_ms: None,
        budget_max_ms: None,
        budget_result: BudgetResult::ReportOnly,
        notes,
        dataset_metadata: PerfDatasetMetadata {
            dataset_id: request.dataset_id.clone(),
            dataset_shape,
            generated: request.reuse_dataset.is_none(),
            reused_from: request
                .reuse_dataset
                .as_ref()
                .map(|path| path.display().to_string()),
            rough_row_count_marker: None,
            account_count: None,
            address_count: None,
            sync_active: false,
        },
    })
}

fn placeholder_notes(request: &PerfRunRequest) -> String {
    let mut notes =
        "Placeholder scenario executed; real HTTP timing lands in later phases.".to_string();

    if request.reuse_dataset.is_some() {
        notes.push_str(" Reused dataset path supplied.");
    }

    notes
}

fn persist_result(output_dir: &Path, record: &PerfResultRecord) -> Result<PerfPaths, PerfError> {
    create_dir_all(output_dir).map_err(|err| {
        PerfError::io(format!(
            "failed to create perf output directory {}: {err}",
            output_dir.display()
        ))
    })?;
    let run_path = output_dir.join(RUN_FILENAME);
    let summary_path = output_dir.join(SUMMARY_FILENAME);

    let json_record =
        serde_json::to_string_pretty(record).map_err(|err| PerfError::Json(err.to_string()))?;
    write(&run_path, format!("{json_record}\n")).map_err(|err| {
        PerfError::io(format!(
            "failed to write perf run file {}: {err}",
            run_path.display()
        ))
    })?;

    let summary = render_summary(record, output_dir, &run_path);
    write(&summary_path, summary).map_err(|err| {
        PerfError::io(format!(
            "failed to write perf summary file {}: {err}",
            summary_path.display()
        ))
    })?;

    Ok(PerfPaths {
        output_dir: output_dir.to_path_buf(),
        run_path,
        summary_path,
    })
}

fn render_summary(record: &PerfResultRecord, output_dir: &Path, run_path: &Path) -> String {
    format!(
        "# Perf Run Summary\n\n- run_id: `{}`\n- timestamp_utc: `{}`\n- scenario_id: `{}`\n- scenario_name: `{}`\n- mode: `{}`\n- dataset_id: `{}`\n- dataset_shape: `{}`\n- budget_result: `{}`\n- notes: {}\n- run_dir: `{}`\n- run_file: `{}`\n",
        record.run_id.as_str(),
        record.timestamp_utc,
        record.scenario_id.as_str(),
        record.scenario_name.as_str(),
        record.mode.as_cli_value(),
        record.dataset_id.as_str(),
        record.dataset_shape.as_str(),
        budget_result_label(record.budget_result),
        record.notes,
        output_dir.display(),
        run_path.display(),
    )
}

fn print_summary(record: &PerfResultRecord, paths: &PerfPaths) {
    println!("BitGarth perf run complete");
    println!("run_id          {}", record.run_id.as_str());
    println!("timestamp_utc   {}", record.timestamp_utc);
    println!("scenario_id     {}", record.scenario_id.as_str());
    println!("mode            {}", record.mode.as_cli_value());
    println!("dataset_id      {}", record.dataset_id.as_str());
    println!(
        "budget_result   {}",
        budget_result_label(record.budget_result)
    );
    println!("run_dir         {}", paths.output_dir.display());
    println!("run_file        {}", paths.run_path.display());
    println!("summary_file    {}", paths.summary_path.display());
}

fn budget_result_label(result: BudgetResult) -> &'static str {
    match result {
        BudgetResult::Passed => "passed",
        BudgetResult::Failed => "failed",
        BudgetResult::ReportOnly => "report_only",
    }
}

fn enforce_budget_result(record: &PerfResultRecord) -> Result<(), PerfError> {
    match record.budget_result {
        BudgetResult::Failed => Err(PerfError::BudgetFailed(format!(
            "perf budget failed for scenario '{}' in mode '{}'",
            record.scenario_id.as_str(),
            record.mode.as_cli_value()
        ))),
        BudgetResult::Passed | BudgetResult::ReportOnly => Ok(()),
    }
}

fn current_build_profile() -> String {
    if cfg!(debug_assertions) {
        "debug".to_string()
    } else {
        "release".to_string()
    }
}

fn current_git_metadata() -> PerfGitMetadata {
    let git_commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_string())
        .filter(|stdout| !stdout.is_empty());

    let git_dirty = Command::new("git")
        .args(["status", "--short"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| !stdout.trim().is_empty());

    PerfGitMetadata {
        git_commit,
        git_dirty,
    }
}

fn print_usage() {
    println!("Usage:");
    println!(
        "  BitGarth perf run [--mode in-process|real-server] [--base-url <loopback-url>] [--scenario <id>] [--dataset <id>] [--timestamp <rfc3339-utc>] [--output-dir <path>] [--reuse-dataset <path>] [--warmup <count>] [--iterations <count>] [--concurrency <count>]"
    );
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn parse_real_server_requires_and_normalizes_base_url() {
        let request = parse_run_command(&[
            OsString::from("--mode"),
            OsString::from("real-server"),
            OsString::from("--base-url"),
            OsString::from("HTTP://LOCALHOST:8081/api%20path"),
        ])
        .expect("real-server arguments should parse");

        assert_eq!(request.mode, PerfMode::RealServer);
        assert_eq!(
            request.base_url.as_deref(),
            Some("http://localhost:8081/api%20path/")
        );
    }

    #[test]
    fn parse_real_server_rejects_missing_base_url() {
        let error = parse_run_command(&[OsString::from("--mode"), OsString::from("real-server")])
            .expect_err("real-server mode should require --base-url");

        assert_eq!(
            error,
            PerfError::Usage("real-server mode requires --base-url".to_string())
        );
    }

    #[test]
    fn parse_in_process_rejects_base_url() {
        let error = parse_run_command(&[
            OsString::from("--base-url"),
            OsString::from("http://127.0.0.1:8081/"),
        ])
        .expect_err("in-process mode should reject --base-url");

        assert_eq!(
            error,
            PerfError::Usage("--base-url is only valid in real-server mode".to_string())
        );
    }

    #[test]
    fn normalize_base_url_rejects_unsupported_scheme_and_non_loopback_host() {
        assert_eq!(
            normalize_base_url("ftp://localhost:8081").expect_err("ftp should be rejected"),
            PerfError::Usage("--base-url must use http or https: ftp://localhost:8081".to_string())
        );
        assert_eq!(
            normalize_base_url("https://example.com").expect_err("public host should be rejected"),
            PerfError::Usage(
                "--base-url must point to 127.0.0.1 or localhost: https://example.com".to_string()
            )
        );
    }

    #[test]
    fn normalize_base_url_rejects_malformed_url() {
        let error = normalize_base_url("not a url").expect_err("malformed URL should be rejected");
        assert!(
            matches!(error, PerfError::Usage(message) if message.starts_with("invalid --base-url 'not a url':"))
        );
    }

    #[test]
    fn normalize_base_url_rejects_credentials_query_and_fragment() {
        for value in [
            "http://user:password@localhost:8081",
            "http://localhost:8081?key=value",
            "http://localhost:8081#fragment",
        ] {
            assert_eq!(
                normalize_base_url(value).expect_err("URL components should be rejected"),
                PerfError::Usage(format!(
                    "--base-url must not contain credentials, query, or fragment: {value}"
                ))
            );
        }
    }

    #[test]
    fn parse_mode_accepts_expected_values() {
        assert_eq!(
            PerfMode::parse("in-process").expect("valid"),
            PerfMode::InProcess
        );
        assert_eq!(
            PerfMode::parse("real_server").expect("valid"),
            PerfMode::RealServer
        );
    }

    #[test]
    fn parse_mode_rejects_unknown_value() {
        let err = PerfMode::parse("other").expect_err("mode should be rejected");
        assert_eq!(
            err,
            PerfError::Usage(
                "unsupported mode 'other', expected in-process or real-server".to_string()
            )
        );
    }

    #[test]
    fn persist_result_writes_run_and_summary_files() {
        let temp_dir = std::env::temp_dir().join(format!("bitgarth-perf-{}", Ulid::new()));
        let record = PerfResultRecord {
            run_id: PerfRunId::new(),
            timestamp_utc: "2026-03-07T10:00:00Z".to_string(),
            scenario_id: ScenarioId::placeholder(),
            scenario_name: ScenarioName::placeholder(),
            mode: PerfMode::InProcess,
            dataset_id: DatasetId::placeholder(),
            dataset_shape: DatasetShape::placeholder(),
            build_profile: "release".to_string(),
            git_commit: Some("abc123".to_string()),
            git_dirty: Some(false),
            app_version: "0.1.0".to_string(),
            endpoint_or_flow: "placeholder".to_string(),
            concurrency: 1,
            warmup_iterations: 0,
            measured_iterations: 1,
            median_ms: 0.0,
            p95_ms: 0.0,
            max_ms: 0.0,
            min_ms: 0.0,
            error_count: 0,
            success_count: 1,
            budget_median_ms: None,
            budget_p95_ms: None,
            budget_max_ms: None,
            budget_result: BudgetResult::ReportOnly,
            notes: "placeholder".to_string(),
            dataset_metadata: PerfDatasetMetadata {
                dataset_id: DatasetId::placeholder(),
                dataset_shape: DatasetShape::placeholder(),
                generated: true,
                reused_from: None,
                rough_row_count_marker: None,
                account_count: None,
                address_count: None,
                sync_active: false,
            },
        };

        let paths = persist_result(&temp_dir, &record).expect("persist should succeed");
        assert!(paths.run_path.exists(), "run file should exist");
        assert!(paths.summary_path.exists(), "summary file should exist");
        let run_json =
            std::fs::read_to_string(&paths.run_path).expect("run JSON should be readable");
        assert!(!run_json.contains(concat!("mock_request", "_counts")));
        assert!(!run_json.contains(concat!("mocks", "_used")));
    }

    #[test]
    fn enforce_budget_result_rejects_failed_budgets() {
        let record = PerfResultRecord {
            run_id: PerfRunId::new(),
            timestamp_utc: "2026-03-07T10:00:00Z".to_string(),
            scenario_id: ScenarioId::parse("wallets-large-read".to_string())
                .expect("scenario id should parse"),
            scenario_name: ScenarioName("Wallets Large Read".to_string()),
            mode: PerfMode::InProcess,
            dataset_id: DatasetId::parse("wallets-many-accounts".to_string())
                .expect("dataset id should parse"),
            dataset_shape: DatasetShape("wallets-24x3".to_string()),
            build_profile: "debug".to_string(),
            git_commit: None,
            git_dirty: None,
            app_version: "0.1.0".to_string(),
            endpoint_or_flow: "GET /_app/user/wallets".to_string(),
            concurrency: 1,
            warmup_iterations: 0,
            measured_iterations: 1,
            median_ms: 100.0,
            p95_ms: 100.0,
            max_ms: 100.0,
            min_ms: 100.0,
            error_count: 0,
            success_count: 1,
            budget_median_ms: Some(10.0),
            budget_p95_ms: Some(20.0),
            budget_max_ms: Some(30.0),
            budget_result: BudgetResult::Failed,
            notes: "failed".to_string(),
            dataset_metadata: PerfDatasetMetadata {
                dataset_id: DatasetId::parse("wallets-many-accounts".to_string())
                    .expect("dataset id should parse"),
                dataset_shape: DatasetShape("wallets-24x3".to_string()),
                generated: true,
                reused_from: None,
                rough_row_count_marker: Some(72),
                account_count: Some(72),
                address_count: Some(72),
                sync_active: false,
            },
        };

        let error = enforce_budget_result(&record).expect_err("failed budget should error");
        assert_eq!(
            error,
            PerfError::BudgetFailed(
                "perf budget failed for scenario 'wallets-large-read' in mode 'in-process'"
                    .to_string()
            )
        );
        assert_eq!(budget_result_label(BudgetResult::Failed), "failed");
        assert_eq!(budget_result_label(BudgetResult::Passed), "passed");
    }
}
