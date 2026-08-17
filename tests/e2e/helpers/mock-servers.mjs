import { createServer } from "node:http";
import { startMockEtherscanServer } from "./mock-etherscan.mjs";
import { startMockMempoolServer } from "./mock-mempool.mjs";

function parseDelay(options, key) {
  const raw = options?.[key] ?? 0;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed < 0) {
    throw new Error(`${key} must be a non-negative number, got: ${raw}`);
  }
  return parsed;
}

function parsePositiveInteger(options, key, defaultValue) {
  const raw = options?.[key] ?? defaultValue;
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`${key} must be a positive integer, got: ${raw}`);
  }
  return parsed;
}

function parseBoolean(options, key) {
  const value = options?.[key];
  return value === true;
}

export async function startMockServers(options = {}) {
  const mempoolDelayMs = parseDelay(options, "mempoolDelayMs");
  const etherscanDelayMs = parseDelay(options, "etherscanDelayMs");
  const etherscanTransactionCount = parsePositiveInteger(
    options,
    "etherscanTransactionCount",
    2,
  );
  const etherscanDynamicAddresses = parseBoolean(options, "etherscanDynamicAddresses");
  const etherscanNativeBalanceWei = options?.etherscanNativeBalanceWei;
  const etherscanKnownAddressTransactions = options?.etherscanKnownAddressTransactions;
  const mempoolAddressData = options?.mempoolAddressData ?? {};
  const mempool = await startMockMempoolServer({
    responseDelayMs: mempoolDelayMs,
    addressData: mempoolAddressData,
  });

  try {
    const etherscan = await startMockEtherscanServer({
      responseDelayMs: etherscanDelayMs,
      transactionCount: etherscanTransactionCount,
      dynamicAddresses: etherscanDynamicAddresses,
      nativeBalanceWei: etherscanNativeBalanceWei,
      knownAddressTransactions: etherscanKnownAddressTransactions,
    });

    let adminServer;

    const status = () => ({
      mempool: mempool.status(),
      etherscan: etherscan.status(),
    });

    adminServer = createServer((req, res) => {
      const url = new URL(req.url ?? "/", "http://127.0.0.1");

      if (req.method === "GET" && url.pathname === "/status") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(status()));
        return;
      }

      if (req.method === "POST" && url.pathname === "/shutdown") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ ok: true }));
        setImmediate(() => {
          close().catch((error) => {
            console.error("Failed to close mock servers during shutdown", error);
            process.exitCode = 1;
          });
        });
        return;
      }

      res.writeHead(404, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "not found" }));
    });

    await new Promise((resolve, reject) => {
      adminServer.once("error", reject);
      adminServer.listen(0, "127.0.0.1", resolve);
    });

    const address = adminServer.address();
    if (!address || typeof address === "string") {
      throw new Error("Mock admin server failed to bind to an IPv4 socket");
    }

    async function close() {
      const closeServer = (server) =>
        new Promise((resolve, reject) => {
          server.close((error) => {
            if (error) {
              reject(error);
              return;
            }
            resolve();
          });
        });

      await Promise.all([closeServer(adminServer), mempool.close(), etherscan.close()]);
    }

    return {
      adminUrl: `http://127.0.0.1:${address.port}/`,
      close,
      etherscan,
      mempool,
      status,
    };
  } catch (error) {
    await mempool.close();
    throw error;
  }
}
