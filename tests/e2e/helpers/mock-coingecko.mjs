import { createServer } from "node:http";

// Mock CoinGecko `/api/v3/simple/price` endpoint for the screenshot/E2E harness.
// Returns fixed, fairly-recent spot prices so balance conversion is deterministic
// and never depends on (or gets rate-limited by) the real CoinGecko API.
//
// Prices are keyed by CoinGecko id. They are intentionally static — "fairly
// recent" is good enough for a showcase screenshot; freshness is not a goal.
// Values are plausible mid-2026 quotes; the same numbers are returned for any
// requested `vs_currencies` (only EUR is exercised by the screenshot run).
const PRICES = {
  bitcoin: 92000,
  ethereum: 3200,
  monero: 280,
};

export async function startMockCoingeckoServer({ port = 0 } = {}) {
  let requestCount = 0;

  const server = createServer((req, res) => {
    requestCount += 1;
    const url = new URL(req.url ?? "/", "http://127.0.0.1");

    if (url.pathname === "/api/v3/simple/price") {
      const ids = (url.searchParams.get("ids") ?? "")
        .split(",")
        .map((id) => id.trim())
        .filter(Boolean);
      const currencies = (url.searchParams.get("vs_currencies") ?? "usd")
        .split(",")
        .map((c) => c.trim().toLowerCase())
        .filter(Boolean);

      const body = {};
      for (const id of ids) {
        if (!(id in PRICES)) continue;
        const byCurrency = {};
        for (const currency of currencies) {
          byCurrency[currency] = PRICES[id];
        }
        body[id] = byCurrency;
      }

      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
      return;
    }

    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "not found" }));
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolve);
  });

  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("Mock CoinGecko server failed to bind to an IPv4 socket");
  }

  // The Rust client joins request paths onto this base, so it must include the
  // `/api/v3/` prefix and a trailing slash.
  const baseUrl = `http://127.0.0.1:${address.port}/api/v3/`;

  return {
    baseUrl,
    prices: PRICES,
    requestCount: () => requestCount,
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
