import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

test("server launcher preserves output, environment, and exit status", (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "bitgarth-server-log-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const logPath = path.join(directory, "server log.txt");
  const result = spawnSync(process.execPath, [
    "tests/e2e/helpers/run-logged.mjs",
    process.execPath,
    "-e",
    'process.stdout.write(process.env.LAUNCHER_TEST_VALUE); process.stderr.write("stderr\\n"); process.exitCode = 7;',
  ], {
    encoding: "utf8",
    env: { ...process.env, BITGARTH_E2E_LOG_PATH: logPath, LAUNCHER_TEST_VALUE: "stdout\n" },
  });

  assert.equal(result.status, 7, result.stderr);
  assert.equal(result.stdout, "stdout\n");
  assert.equal(result.stderr, "stderr\n");
  const log = fs.readFileSync(logPath, "utf8");
  assert.ok(log.includes("stdout\n"));
  assert.ok(log.includes("stderr\n"));
});

test("server launcher reports a missing executable as a failure", () => {
  const result = spawnSync(process.execPath, [
    "tests/e2e/helpers/run-logged.mjs",
    "./nonexistent-bitgarth-test-server",
  ], { encoding: "utf8" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /ENOENT/);
});
