import { createServer } from "node:http";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const fixture = require("../fixtures/mempool-fixture.json");

function responseDelayMs(options) {
  const raw = options?.responseDelayMs ?? 0;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed < 0) {
    throw new Error(`Mock mempool response delay must be a non-negative number, got: ${raw}`);
  }
  return parsed;
}

export async function startMockMempoolServer(options = {}) {
  let requestCount = 0;
  const delayMs = responseDelayMs(options);
  // Optional per-address data override (address -> { stats, txs }). Used by the
  // screenshot harness to serve a realistic BTC balance without mutating the
  // shared fixture. `stats` populates `GET /api/address/{addr}` (the balance
  // read path: chain_stats.funded_txo_sum - spent_txo_sum); `txs` populates
  // `GET /api/address/{addr}/txs`. Specs that omit it are unaffected.
  const addressData = options?.addressData ?? {};

  const server = createServer((req, res) => {
    requestCount += 1;
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const respond = () => {
      if (url.pathname === "/api/blocks/tip/height") {
        res.writeHead(200, { "content-type": "text/plain" });
        res.end(String(fixture.chainTipHeight));
        return;
      }

      const addressTxsMatch = url.pathname.match(/^\/api\/address\/([^/]+)\/txs$/);
      if (addressTxsMatch) {
        const address = addressTxsMatch[1];

        if (Object.prototype.hasOwnProperty.call(addressData, address)) {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify(addressData[address].txs ?? []));
          return;
        }

        if (address === fixture.knownAddress) {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify(fixture.transactions));
        } else {
          res.writeHead(200, { "content-type": "application/json" });
          res.end("[]");
        }
        return;
      }

      const addressMatch = url.pathname.match(/^\/api\/address\/([^/]+)$/);
      if (addressMatch) {
        const address = addressMatch[1];
        if (Object.prototype.hasOwnProperty.call(addressData, address)) {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(
            JSON.stringify({
              address,
              chain_stats: addressData[address].stats,
              mempool_stats: { funded_txo_sum: 0, spent_txo_sum: 0, tx_count: 0 },
            }),
          );
          return;
        }
        if (address === fixture.knownAddress) {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(
            JSON.stringify({
              address,
              chain_stats: fixture.knownAddressStats,
              mempool_stats: { funded_txo_sum: 0, spent_txo_sum: 0, tx_count: 0 },
            }),
          );
          return;
        }
        res.writeHead(404, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: "not found" }));
        return;
      }

      res.writeHead(404, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: "not found" }));
    };

    if (delayMs > 0) {
      setTimeout(respond, delayMs);
      return;
    }

    respond();
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });

  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("Mock mempool server failed to bind to an IPv4 socket");
  }

  return {
    baseUrl: `http://127.0.0.1:${address.port}/`,
    requestCount: () => requestCount,
    status: () => ({
      baseUrl: `http://127.0.0.1:${address.port}/`,
      requestCount,
      responseDelayMs: delayMs,
    }),
    close: () =>
      new Promise((resolve, reject) => {
        server.close((err) => {
          if (err) {
            reject(err);
            return;
          }
          resolve();
        });
      }),
  };
}
