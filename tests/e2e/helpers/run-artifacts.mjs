import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";

// ponytail: 24h lock ceiling; use process birth identity if test runs ever span days.
const RUN_LOCK_MAX_AGE_MS = 24 * 60 * 60 * 1000;

function pad(value) {
  return String(value).padStart(2, "0");
}

export function formatRunTimestamp(now = new Date()) {
  return [
    now.getUTCFullYear(),
    pad(now.getUTCMonth() + 1),
    pad(now.getUTCDate()),
  ].join("-") +
    `T${pad(now.getUTCHours())}-${pad(now.getUTCMinutes())}-${pad(now.getUTCSeconds())}Z`;
}

export function formatRunTimestampUtc(now = new Date()) {
  return [
    now.getUTCFullYear(),
    pad(now.getUTCMonth() + 1),
    pad(now.getUTCDate()),
  ].join("-") +
    `T${pad(now.getUTCHours())}:${pad(now.getUTCMinutes())}:${pad(now.getUTCSeconds())}Z`;
}

export function timestampUtcFromRunTimestamp(timestamp) {
  const match = timestamp.match(
    /^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-(\d{2})Z$/,
  );
  if (!match) {
    return timestamp;
  }

  return `${match[1]}T${match[2]}:${match[3]}:${match[4]}Z`;
}

function readLock(lockPath) {
  try {
    const raw = fs.readFileSync(lockPath, "utf8").trim();
    const sep = raw.indexOf(":");
    if (sep === -1) {
      return null;
    }

    const pid = Number(raw.slice(0, sep));
    const runDir = raw.slice(sep + 1);
    if (!Number.isFinite(pid) || runDir.length === 0) {
      return null;
    }

    return { pid, runDir };
  } catch {
    return null;
  }
}

function isProcessAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export function testTypeRoot(testType) {
  return path.resolve("test-results", testType);
}

export function runDirFor(testType, timestamp, { rootDir = null } = {}) {
  return path.join(rootDir ?? testTypeRoot(testType), timestamp);
}

export function claimRunDirectory({
  testType,
  lockFilename = ".current-run-dir",
  now = new Date(),
  rootDir = null,
} = {}) {
  const resolvedRootDir = path.resolve(rootDir ?? testTypeRoot(testType));
  fs.mkdirSync(resolvedRootDir, { recursive: true });

  const lockPath = path.join(resolvedRootDir, lockFilename);
  const currentPid = process.pid;
  const lock = fs.existsSync(lockPath) ? readLock(lockPath) : null;
  let lockAgeMs = Number.POSITIVE_INFINITY;
  if (lock) {
    lockAgeMs = now.getTime() - fs.statSync(lockPath).mtimeMs;
  }

  if (
    lock &&
    lockAgeMs >= 0 &&
    lockAgeMs <= RUN_LOCK_MAX_AGE_MS &&
    isProcessAlive(lock.pid)
  ) {
    const runDir = path.resolve(lock.runDir);
    fs.mkdirSync(runDir, { recursive: true });
    const timestamp = path.basename(runDir);
    return {
      lockPath,
      rootDir: resolvedRootDir,
      runDir,
      timestamp,
      timestampUtc: timestampUtcFromRunTimestamp(timestamp),
    };
  }

  const timestamp = formatRunTimestamp(now);
  const runDir = runDirFor(testType, timestamp, { rootDir: resolvedRootDir });

  if (fs.existsSync(runDir)) {
    throw new Error(
      `run directory already exists for ${testType}: ${runDir}`,
    );
  }

  fs.mkdirSync(runDir, { recursive: false });
  fs.writeFileSync(lockPath, `${currentPid}:${runDir}\n`, "utf8");

  return {
    lockPath,
    rootDir: resolvedRootDir,
    runDir,
    timestamp,
    timestampUtc: formatRunTimestampUtc(now),
  };
}

export function clearRunDirectoryLock({
  testType,
  lockFilename = ".current-run-dir",
} = {}) {
  const lockPath = path.join(testTypeRoot(testType), lockFilename);
  try {
    fs.unlinkSync(lockPath);
  } catch {
    // Nothing to clean up.
  }
}

export async function ensureRunSubdirs(
  runDir,
  { artifacts = false, logs = false, data = false } = {},
) {
  await fsp.mkdir(runDir, { recursive: true });

  const created = { runDir };
  if (artifacts) {
    created.artifactsDir = path.join(runDir, "artifacts");
    await fsp.mkdir(created.artifactsDir, { recursive: true });
  }
  if (logs) {
    created.logsDir = path.join(runDir, "logs");
    await fsp.mkdir(created.logsDir, { recursive: true });
  }
  if (data) {
    created.dataDir = path.join(runDir, "data");
    await fsp.mkdir(created.dataDir, { recursive: true });
  }

  return created;
}

export function ensureRunSubdirsSync(
  runDir,
  { artifacts = false, logs = false, data = false } = {},
) {
  fs.mkdirSync(runDir, { recursive: true });

  const created = { runDir };
  if (artifacts) {
    created.artifactsDir = path.join(runDir, "artifacts");
    fs.mkdirSync(created.artifactsDir, { recursive: true });
  }
  if (logs) {
    created.logsDir = path.join(runDir, "logs");
    fs.mkdirSync(created.logsDir, { recursive: true });
  }
  if (data) {
    created.dataDir = path.join(runDir, "data");
    fs.mkdirSync(created.dataDir, { recursive: true });
  }

  return created;
}

export async function writeRunMetadata(runDir, metadata) {
  const runMetadataPath = path.join(runDir, "run.json");
  await fsp.writeFile(
    runMetadataPath,
    `${JSON.stringify(metadata, null, 2)}\n`,
    "utf8",
  );
  return runMetadataPath;
}

export function writeRunMetadataSync(runDir, metadata) {
  const runMetadataPath = path.join(runDir, "run.json");
  fs.writeFileSync(
    runMetadataPath,
    `${JSON.stringify(metadata, null, 2)}\n`,
    "utf8",
  );
  return runMetadataPath;
}

export async function writeRunSummary(runDir, summary) {
  const summaryPath = path.join(runDir, "summary.md");
  await fsp.writeFile(summaryPath, summary, "utf8");
  return summaryPath;
}

export function writeRunSummarySync(runDir, summary) {
  const summaryPath = path.join(runDir, "summary.md");
  fs.writeFileSync(summaryPath, summary, "utf8");
  return summaryPath;
}
