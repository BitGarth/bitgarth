import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { claimRunDirectory } from "./run-artifacts.mjs";

test("stale run lock is not reused after its PID is recycled", (t) => {
  const rootDir = fs.mkdtempSync(path.join(os.tmpdir(), "bitgarth-run-artifacts-"));
  t.after(() => fs.rmSync(rootDir, { recursive: true, force: true }));

  const staleRunDir = path.join(rootDir, "2026-08-12T20-58-51Z");
  const lockPath = path.join(rootDir, ".current-run-dir");
  fs.mkdirSync(staleRunDir);
  fs.writeFileSync(lockPath, `${process.pid}:${staleRunDir}\n`);
  const staleTime = new Date("2026-08-12T20:58:51Z");
  fs.utimesSync(lockPath, staleTime, staleTime);

  const claimed = claimRunDirectory({
    testType: "e2e",
    rootDir,
    now: new Date("2026-08-16T00:00:00Z"),
  });

  assert.equal(claimed.timestamp, "2026-08-16T00-00-00Z");
});

test("worker config loading preserves the active server log", (t) => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "bitgarth-playwright-config-"));
  t.after(() => fs.rmSync(tempDir, { recursive: true, force: true }));

  const configUrl = pathToFileURL(path.resolve("playwright.config.mjs")).href;
  const script = `
    const fs = await import("node:fs");
    const path = await import("node:path");
    const root = process.argv[1];
    const configUrl = process.argv[2];
    const runRoot = path.join(root, "test-results", "e2e");
    const runDir = path.join(runRoot, "2026-07-26T00-00-00Z");
    const logsDir = path.join(runDir, "logs");
    fs.mkdirSync(logsDir, { recursive: true });
    fs.writeFileSync(path.join(runRoot, ".current-run-dir"), process.pid + ":" + runDir + "\\n");
    fs.writeFileSync(path.join(logsDir, "server.log"), "sentinel\\n");
    process.chdir(root);
    process.env.TEST_WORKER_INDEX = "0";
    await import(configUrl);
    process.stdout.write(fs.readFileSync(path.join(logsDir, "server.log"), "utf8"));
  `;

  const result = spawnSync(process.execPath, ["--input-type=module", "-e", script, tempDir, configUrl], {
    encoding: "utf8",
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "sentinel\n");
});
