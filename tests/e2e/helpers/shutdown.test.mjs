import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import net from "node:net";
import { DatabaseSync } from "node:sqlite";
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

async function startServer(projectDir = mkdtempSync(path.join(os.tmpdir(), "bitgarth-shutdown-")), env = {}) {
  const port = await freePort();
  const child = spawn(serverBinary, [], {
    env: { ...process.env, RUST_LOG: "info", ...env, BITGARTH_PROJECT_DIR: projectDir, IP: "127.0.0.1", PORT: String(port) },
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

test("restart collects shared prices with both household users signed out", { timeout: 40_000 },
  () => checkSignedOutPriceCollection("docker"));
test("hosted restart collects every priced pair with everyone signed out", { timeout: 40_000 },
  () => checkSignedOutPriceCollection("hosted"));

async function checkSignedOutPriceCollection(channel) {
  const project = mkdtempSync(path.join(os.tmpdir(), "bitgarth-price-restart-"));
  const seen = new Set();
  let requestCount = 0;
  const provider = http.createServer((req, res) => {
    requestCount++;
    const url = new URL(req.url, "http://127.0.0.1");
    const asset = url.pathname.match(/\/coins\/([^/]+)\/market_chart\/range$/)?.[1];
    const currency = url.searchParams.get("vs_currency");
    seen.add(`${asset}/${currency}`);
    const from = Number(url.searchParams.get("from"));
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ prices: [[from * 1000, 20]] }));
  });
  await new Promise((resolve) => provider.listen(0, "127.0.0.1", resolve));
  const env = {
    BITGARTH_CHANNEL: channel,
    BITGARTH_COINGECKO_BASE_URL: `http://127.0.0.1:${provider.address().port}/api/v3/`,
    RUST_LOG: "info,bitgarth_app::tasks=debug",
  };
  let app;
  let prices;
  try {
    app = await startServer(project, env);
    for (let i = 0; i < 100 && !app.output().includes("shared price history run"); i++) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const idle = app.output().split("\n").find((line) => line.includes("shared price history run"));
    const registration = app.output().split("\n").find((line) =>
      line.includes("registered background job") && line.includes("job_id=SharedPriceHistory"));
    const firstRun = registration?.match(/next_run_at_utc=Some\(([^)]+)\)/)?.[1];
    assert.match(idle ?? "", /reason="?no_opt_in/);
    assert.ok(firstRun && Date.parse(firstRun) <= Date.parse(idle.split(" ")[0]),
      `Incorrect first run reported at startup: ${registration}`);
    assert.match(idle, /requests_attempted=0/);
    assert.match(idle, /rows_written=0/);
    assert.equal(requestCount, 0);
    assert.match(app.output(), /INFO[^\n]*migrations applied/);
    app.child.kill("SIGTERM");
    await within(app.exited, 10_000);
    const db = new DatabaseSync(path.join(project, "app/data/app.db"));
    try {
      for (const [id, name, consent] of [
        ["01K2EZ4N4G0W0EPF9YXRJZD9WQ", "alice", 0],
        ["01K2EZ4N4G0W0EPF9YXRJZD9WR", "bob", 1],
      ]) {
        db.prepare("INSERT INTO users(user_id,username,created_at,updated_at) VALUES(?,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").run(id, name);
        db.prepare("INSERT INTO app_user_preferences VALUES(?,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").run(id, consent);
      }
    } finally {
      db.close();
    }
    prices = new DatabaseSync(path.join(project, "app/data/prices/prices.db"));
    for (const [asset, currency] of [["bitcoin", "USD"], ["solana", "EUR"], ["ethereum", "EUR"]]) {
      prices.prepare(`INSERT INTO current_price_cache
        (asset_id, quote_currency, provider, provider_asset_id, price, license_scope, retrieved_at, created_at, updated_at)
        VALUES (?, ?, 'coingecko', ?, '10', 'public_keyless', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)`).run(asset, currency, asset);
    }
    const expectedPairs = ["bitcoin/usd", "ethereum/eur", "solana/eur"];
    app = await startServer(project, env);
    for (let i = 0; i < 100; i++) {
      const rows = prices.prepare("SELECT DISTINCT asset_id FROM price_points").all();
      if (rows.length === expectedPairs.length) break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert.deepEqual([...seen].sort(), expectedPairs);
    assert.equal(prices.prepare("SELECT COUNT(DISTINCT asset_id) AS n FROM price_points").get().n, expectedPairs.length);
    app.child.kill("SIGTERM");
    await within(app.exited, 10_000);
    const summaries = app.output().split("\n").filter((line) => line.includes("shared price history run"));
    assert.equal(summaries.length, 1, app.output());
    const [summary] = summaries;
    assert.doesNotMatch(app.output(), /INFO[^\n]*(?:WAL mode enabled|db: opening database|no migrations applied)/);
    const rows = prices.prepare("SELECT COUNT(*) AS n FROM price_points").get().n;
    assert.equal(Number(summary.match(/rows_written=(\d+)/)?.[1]), rows);
    assert.equal(Number(summary.match(/requests_attempted=(\d+)/)?.[1]), requestCount);
    assert.equal(Number(summary.match(/requests_succeeded=(\d+)/)?.[1]), requestCount);
    assert.doesNotMatch(summary, /bitcoin|solana|alice|bob|user_id|account_id|provider_asset_id/i);
  } finally {
    if (app?.child.exitCode === null) {
      app.child.kill("SIGTERM");
      await within(app.exited, 10_000);
    }
    prices?.close();
    await new Promise((resolve) => provider.close(resolve));
    rmSync(project, { recursive: true, force: true });
  }
}

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
