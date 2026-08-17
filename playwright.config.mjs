import fs from "node:fs";
import path from "node:path";
import { defineConfig, devices } from "@playwright/test";

import {
  claimRunDirectory,
  ensureRunSubdirsSync,
  writeRunMetadataSync,
} from "./tests/e2e/helpers/run-artifacts.mjs";

function shellQuote(value) {
  return `'${value.replaceAll("'", `'\"'\"'`)}'`;
}

const {
  runDir,
  timestamp: runTimestamp,
  timestampUtc,
} = claimRunDirectory({ testType: "e2e" });
const { artifactsDir, logsDir, dataDir } = ensureRunSubdirsSync(runDir, {
  artifacts: true,
  logs: true,
  data: true,
});

// e2e always uses its own per-run isolated project dir. An ambient
// BITGARTH_PROJECT_DIR (e.g. from a personal .env.local) must not leak test
// users into a real project dir, so it is intentionally ignored here.
const e2eProjectDir = dataDir;
const serverLogPath = path.join(logsDir, "server.log");
const timelineLogPath = path.join(logsDir, "test-timeline.log");
const playwrightOutputDir = path.join(artifactsDir, "playwright-output");
const playwrightReportDir = path.join(artifactsDir, "playwright-report");

process.env.BITGARTH_PROJECT_DIR = e2eProjectDir;
process.env.__BITGARTH_E2E_RUN_DIR = runDir;
process.env.__BITGARTH_E2E_RUN_TIMESTAMP = runTimestamp;

fs.mkdirSync(playwrightOutputDir, { recursive: true });
fs.mkdirSync(playwrightReportDir, { recursive: true });
if (process.env.TEST_WORKER_INDEX === undefined) {
  fs.writeFileSync(timelineLogPath, "", "utf8");
  fs.writeFileSync(serverLogPath, "", "utf8");

  writeRunMetadataSync(runDir, {
    test_type: "e2e",
    timestamp_utc: timestampUtc,
    run_dir: path.relative(process.cwd(), runDir),
    project_dir: path.relative(process.cwd(), e2eProjectDir),
    command: "playwright test",
    exit_code: null,
    git_commit: null,
    git_dirty: null,
    notes: "playwright local run",
  });
}

const MOCK_CENTRAL_PORT = 8082;
const mockCentralLogPath = path.join(logsDir, "mock-central.log");

const webServerCommand = [
  `mkdir -p ${shellQuote(runDir)}`,
  "&&",
  "IP=127.0.0.1",
  "PORT=8081",
  "RUST_LOG=debug",
  "BGTRACES=fs",
  `BITGARTH_PROJECT_DIR=${shellQuote(e2eProjectDir)}`,
  `BITGARTH_CENTRAL_BASE_URL=http://127.0.0.1:${MOCK_CENTRAL_PORT}`,
  "BITGARTH_CHANNEL=docker",
  "BITGARTH_PAYMENT_SIGNING_PUBLIC_KEY_B64=O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik",
  "BITGARTH_INSTANCE_NOTICE_INFO=" +
    shellQuote(
      "E2E demo notice with [bitgarth.app](https://bitgarth.app/) and [hello@bitgarth.app](mailto:hello@bitgarth.app).",
    ),
  "RUST_BACKTRACE=1",
  "./target/dx/bitgarth-app/release/web/server",
  "2>&1",
  "|",
  "tee",
  shellQuote(serverLogPath),
].join(" ");

const mockCentralCommand = [
  "node",
  "tests/e2e/helpers/mock-central-cli.mjs",
  `--port=${MOCK_CENTRAL_PORT}`,
  "2>&1",
  "|",
  "tee",
  shellQuote(mockCentralLogPath),
].join(" ");

export default defineConfig({
  globalTeardown: "./tests/e2e/helpers/global-teardown.mjs",
  testDir: "./tests/e2e/specs",
  timeout: 45_000,
  workers: 1,
  fullyParallel: false,
  retries: process.env.CI ? 2 : 0,
  reporter: [
    ["list"],
    ["html", { outputFolder: playwrightReportDir, open: "never" }],
  ],
  outputDir: playwrightOutputDir,
  use: {
    baseURL: "http://127.0.0.1:8081",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  webServer: [
    {
      command: mockCentralCommand,
      url: `http://127.0.0.1:${MOCK_CENTRAL_PORT}/__mock/status`,
      timeout: 30_000,
      reuseExistingServer: false,
    },
    {
      command: webServerCommand,
      url: "http://127.0.0.1:8081/health",
      timeout: 120_000,
      reuseExistingServer: false,
    },
  ],
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
