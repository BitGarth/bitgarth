import fs from "node:fs/promises";
import path from "node:path";

function nowIso() {
  return new Date().toISOString();
}

async function appendLine(filePath, line) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.appendFile(filePath, `${line}\n`, "utf8");
}

function isIgnorableRequestFailure(request, failure) {
  const errorText = failure?.errorText ?? "";
  if (errorText.includes("net::ERR_ABORTED")) return true;
  // Dioxus debug builds embed a hot-reload client that tries /_dioxus
  // against a dev server that isn't running in e2e; ignore its noise.
  if (request.url().includes("/_dioxus")) return true;
  return false;
}

function isIgnorableConsoleError(message) {
  // Same hot-reload client: failed WS handshakes surface as console errors.
  return message.text().includes("/_dioxus");
}

/// dioxus-core reports element-arena corruption ("cannot reclaim ElementId(..)")
/// through its tracing layer, which writes to `console.log` with `%c` styling —
/// never to `console.error`. So it is invisible to `consoleErrors` even though
/// it is the precursor to a WASM `RuntimeError: unreachable` trap.
function isDioxusArenaError(message) {
  return message.text().includes("cannot reclaim ElementId");
}

export async function markTestBoundary(testInfo, phase) {
  const runDir = process.env.__BITGARTH_E2E_RUN_DIR;
  const timeline = runDir
    ? path.join(runDir, "logs", "test-timeline.log")
    : path.resolve("test-results", "e2e", "no-run-context", "logs", "test-timeline.log");
  await appendLine(timeline, `[${nowIso()}] ${phase} ${testInfo.title}`);
}

export async function attachBrowserDiagnostics(page, testInfo) {
  const consoleLogPath = testInfo.outputPath("browser-console.log");

  const consoleErrors = [];
  const arenaErrors = [];
  const pageErrors = [];
  const requestFailures = [];

  page.on("console", (message) => {
    const entry = `[${nowIso()}] [console:${message.type()}] ${message.text()}`;
    void appendLine(consoleLogPath, entry);
    if (isIgnorableConsoleError(message)) {
      return;
    }
    if (message.type() === "error") {
      consoleErrors.push(message.text());
    }
    if (isDioxusArenaError(message)) {
      arenaErrors.push(message.text());
    }
  });

  page.on("pageerror", (error) => {
    // Keep the stack: a bare "RuntimeError: unreachable" from the WASM module
    // says nothing about where it trapped.
    const text = error?.stack ? `${error}\n${error.stack}` : String(error);
    const entry = `[${nowIso()}] [pageerror] ${text}`;
    void appendLine(consoleLogPath, entry);
    pageErrors.push(text);
  });

  page.on("requestfailed", (request) => {
    const failure = request.failure();
    if (isIgnorableRequestFailure(request, failure)) {
      return;
    }
    const text = `${request.method()} ${request.url()} :: ${failure?.errorText ?? "unknown"}`;
    const entry = `[${nowIso()}] [requestfailed] ${text}`;
    void appendLine(consoleLogPath, entry);
    requestFailures.push(text);
  });

  page.on("response", (response) => {
    if (response.status() < 400) {
      return;
    }
    const request = response.request();
    const entry = `[${nowIso()}] [response:${response.status()}] ${request.method()} ${response.url()}`;
    void appendLine(consoleLogPath, entry);
  });

  return { consoleErrors, arenaErrors, pageErrors, requestFailures };
}
