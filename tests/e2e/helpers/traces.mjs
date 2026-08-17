import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

function usersRootDir() {
  const overriddenProjectDir = process.env.BITGARTH_PROJECT_DIR;
  if (overriddenProjectDir && overriddenProjectDir.trim().length > 0) {
    return path.join(overriddenProjectDir.trim(), "users");
  }

  return path.join(
    os.homedir(),
    "Library",
    "Application Support",
    "app.bitgarth.bitgarth",
    "users",
  );
}

async function directoryExists(dirPath) {
  try {
    const stat = await fs.stat(dirPath);
    return stat.isDirectory();
  } catch {
    return false;
  }
}

export async function countHarTraceFiles({ userId = null, label = null } = {}) {
  const root = userId
    ? path.join(usersRootDir(), userId, "traces")
    : usersRootDir();

  if (!(await directoryExists(root))) {
    return 0;
  }

  let count = 0;
  const queue = [root];

  while (queue.length > 0) {
    const current = queue.shift();
    const entries = await fs.readdir(current, { withFileTypes: true });
    for (const entry of entries) {
      const fullPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        queue.push(fullPath);
        continue;
      }
      if (!entry.isFile()) {
        continue;
      }
      if (!entry.name.endsWith(".har")) {
        continue;
      }
      if (label && !entry.name.includes(label)) {
        continue;
      }
      count += 1;
    }
  }

  return count;
}

export async function waitForHarTraceCountIncrease(
  previousCount,
  { userId = null, label = null, timeoutMs = 15_000, pollMs = 200 } = {},
) {
  const deadline = Date.now() + timeoutMs;

  while (Date.now() <= deadline) {
    const current = await countHarTraceFiles({ userId, label });
    if (current > previousCount) {
      return current;
    }
    await new Promise((resolve) => setTimeout(resolve, pollMs));
  }

  return countHarTraceFiles({ userId, label });
}
