import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const serverBinary = path.resolve("target/dx/bitgarth-app/release/web/server");

async function freePort() {
  const server = net.createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function healthy(port) {
  const url = `http://127.0.0.1:${port}/health`;
  for (let attempt = 0; attempt < 200; attempt++) {
    try {
      if ((await fetch(url, { signal: AbortSignal.timeout(500) })).ok) return;
    } catch { /* server is starting */ }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("server did not become healthy");
}

async function within(promise, milliseconds) {
  let timeout;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error("timed out")), milliseconds);
      }),
    ]);
  } finally {
    clearTimeout(timeout);
  }
}

async function startServer(projectDir = mkdtempSync(path.join(os.tmpdir(), "bitgarth-shutdown-"))) {
  const port = await freePort();
  const child = spawn(serverBinary, [], {
    env: { ...process.env, BITGARTH_PROJECT_DIR: projectDir, IP: "127.0.0.1", PORT: String(port), RUST_LOG: "info" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const exited = new Promise((resolve, reject) => {
    child.once("close", (code, signal) => resolve({ code, signal }));
    child.once("error", reject);
  });
  let output = "";
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.on("data", (chunk) => { output += chunk; });
  try {
    await Promise.race([
      healthy(port),
      exited.then((result) => { throw new Error(`server exited during startup: ${JSON.stringify(result)}\n${output}`); }),
    ]);
  } catch (error) {
    cleanupServer({ child, projectDir });
    throw error;
  }
  return { child, exited, port, projectDir, output: () => output };
}

function cleanupServer({ child, projectDir }, removeData = true) {
  if (child.exitCode === null) child.kill("SIGKILL");
  if (removeData) rmSync(projectDir, { recursive: true, force: true });
}

test("SIGTERM drains the web server and exits cleanly", { timeout: 40_000 }, async () => {
  const server = await startServer();
  try {
    server.child.kill("SIGTERM");
    const result = await within(server.exited, 10_000);
    assert.deepEqual(result, { code: 0, signal: null }, server.output());
    assert.match(server.output(), /Shutdown complete/);
    assert.doesNotMatch(server.output(), /Shutdown forced/);
  } finally {
    cleanupServer(server);
  }
});

test("SIGTERM lets an in-flight database write finish and survive restart", { timeout: 40_000 }, async () => {
  const server = await startServer();
  try {
    const legal = readFileSync("src/legal.rs", "utf8");
    const version = (name) => legal.match(new RegExp(`const ${name}: &str = "([^"]+)"`))[1];
    const body = JSON.stringify({
      username: "shutdown-check",
      password: "ShutdownCheck123!",
      legal_acknowledgement: {
        accepted_terms_version: version("TERMS_VERSION"),
        accepted_privacy_version: version("PRIVACY_VERSION"),
      },
    });
    const response = new Promise((resolve, reject) => {
      const request = http.request({
        hostname: "127.0.0.1",
        port: server.port,
        method: "POST",
        path: "/_app/auth/register",
        headers: { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body), Expect: "100-continue" },
      }, (result) => {
        let data = "";
        result.on("data", (chunk) => { data += chunk; });
        result.once("end", () => resolve({ status: result.statusCode, data, cookies: result.headers["set-cookie"] ?? [] }));
      });
      request.once("error", reject);
      request.once("continue", () => {
        request.write(body.slice(0, 1));
        server.child.kill("SIGTERM");
        request.end(body.slice(1));
      });
      request.flushHeaders();
    });
    const result = await within(response, 10_000);
    assert.equal(result.status, 200, result.data);
    const userId = JSON.parse(result.data).user.user_id;
    const cookie = result.cookies.map((value) => value.split(";")[0]).join("; ");
    assert.ok(cookie);
    assert.deepEqual(await within(server.exited, 10_000), { code: 0, signal: null }, server.output());
    assert.match(server.output(), /Shutdown complete/);

    const restarted = await startServer(server.projectDir);
    try {
      const me = await fetch(`http://127.0.0.1:${restarted.port}/_app/auth/me`, {
        headers: { Cookie: cookie },
      });
      assert.equal(me.status, 200);
      assert.equal((await me.json()).user.user_id, userId);

      const events = await fetch(`http://127.0.0.1:${restarted.port}/_app/user/transactions/sync/events`, {
        headers: { Cookie: cookie },
      });
      assert.equal(events.status, 200);
      const stream = events.body.getReader();
      assert.equal((await within(stream.read(), 5_000)).done, false);
      restarted.child.kill("SIGTERM");
      await within((async () => {
        while (!(await stream.read()).done) { /* drain the SSE stream */ }
      })(), 5_000);
      assert.deepEqual(await within(restarted.exited, 10_000), { code: 0, signal: null }, restarted.output());
      assert.match(restarted.output(), /Shutdown complete/);
    } finally {
      cleanupServer(restarted, false);
    }
  } finally {
    cleanupServer(server);
  }
});

test("Docker PID 1 handles SIGTERM", {
  skip: !process.env.BITGARTH_SHUTDOWN_DOCKER_IMAGE,
  timeout: 40_000,
}, async () => {
  const port = await freePort();
  const image = process.env.BITGARTH_SHUTDOWN_DOCKER_IMAGE;
  const started = spawnSync("docker", ["run", "-d", "-p", `127.0.0.1:${port}:8080`, image], { encoding: "utf8" });
  assert.equal(started.status, 0, started.stderr);
  const container = started.stdout.trim();
  try {
    await healthy(port);
    const stopped = spawnSync("docker", ["stop", "--time", "10", container], { encoding: "utf8" });
    assert.equal(stopped.status, 0, stopped.stderr);
    const logs = spawnSync("docker", ["logs", container], { encoding: "utf8" });
    assert.equal(logs.status, 0, logs.stderr);
    assert.match(logs.stdout + logs.stderr, /Shutdown complete/);
  } finally {
    spawnSync("docker", ["rm", "-f", container]);
  }
});
