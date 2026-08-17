import { createServer } from "node:http";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const fixture = require("../fixtures/etherscan-fixture.json");

const knownAddressLower = fixture.knownAddress.toLowerCase();
const SYNTHETIC_BASE_BLOCK_NUMBER = 21_500_100;
const SYNTHETIC_BASE_TIMESTAMP = 1_735_689_600;
const SYNTHETIC_INCOMING_ADDRESS = "0x1111111111111111111111111111111111111111";
const SYNTHETIC_OUTGOING_ADDRESS = "0x2222222222222222222222222222222222222222";
const SYNTHETIC_NATIVE_BALANCE_WEI = "2500000000000000000";

function responseDelayMs(options) {
  const raw = options?.responseDelayMs ?? 0;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed < 0) {
    throw new Error(`Mock etherscan response delay must be a non-negative number, got: ${raw}`);
  }
  return parsed;
}

function transactionCount(options) {
  const raw = options?.transactionCount ?? fixture.transactions.length;
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`Mock etherscan transactionCount must be a positive integer, got: ${raw}`);
  }
  return parsed;
}

function dynamicAddresses(options) {
  return options?.dynamicAddresses === true;
}

function nativeBalanceWei(options) {
  return options?.nativeBalanceWei ?? SYNTHETIC_NATIVE_BALANCE_WEI;
}

function knownAddressTransactions(options) {
  return Array.isArray(options?.knownAddressTransactions)
    ? options.knownAddressTransactions
    : null;
}

function txHash(address, ordinal) {
  const normalizedAddress = address.toLowerCase().replace(/^0x/, "");
  return `0x${normalizedAddress}${ordinal.toString(16).padStart(24, "0")}`;
}

function buildSyntheticTransactions(address, count) {
  const normalizedAddress = address.toLowerCase();
  const rows = [];

  for (let index = 0; index < count; index += 1) {
    const ordinal = index + 1;
    const incoming = index % 2 === 0;
    rows.push({
      blockNumber: String(SYNTHETIC_BASE_BLOCK_NUMBER + index),
      timeStamp: String(SYNTHETIC_BASE_TIMESTAMP + index * 60),
      hash: txHash(normalizedAddress, ordinal),
      from: incoming ? SYNTHETIC_INCOMING_ADDRESS : normalizedAddress,
      to: incoming ? normalizedAddress : SYNTHETIC_OUTGOING_ADDRESS,
      value: incoming ? "500000000000000000" : "1200000000000000000",
      gasPrice: "12000000000",
      gasUsed: "21000",
      isError: "0",
      txreceipt_status: "1",
      nonce: String(index),
    });
  }

  return rows;
}

export async function startMockEtherscanServer(options = {}) {
  let requestCount = 0;
  const delayMs = responseDelayMs(options);
  const txCount = transactionCount(options);
  const serveDynamicAddresses = dynamicAddresses(options);
  const balanceWei = nativeBalanceWei(options);
  const knownTxOverride = knownAddressTransactions(options);

  const server = createServer((req, res) => {
    requestCount += 1;
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const params = url.searchParams;
    const module = params.get("module");
    const action = params.get("action");
    const respond = () => {
      if (module === "proxy" && action === "eth_blockNumber") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            jsonrpc: "2.0",
            id: 1,
            result: fixture.blockNumber,
          }),
        );
        return;
      }

      if (module === "account" && action === "txlist") {
        const address = (params.get("address") ?? "").toLowerCase();
        const sort = (params.get("sort") ?? "asc").toLowerCase();

        if (serveDynamicAddresses && /^0x[0-9a-f]{40}$/.test(address)) {
          const result = buildSyntheticTransactions(address, txCount);
          res.writeHead(200, { "content-type": "application/json" });
          res.end(
            JSON.stringify({
              status: "1",
              message: "OK",
              result: sort === "desc" ? result.reverse() : result,
            }),
          );
          return;
        }

        if (address === knownAddressLower) {
          const result =
            knownTxOverride ??
            (txCount === fixture.transactions.length
              ? fixture.transactions
              : buildSyntheticTransactions(address, txCount));
          const sortedResult = sort === "desc" ? [...result].reverse() : result;
          res.writeHead(200, { "content-type": "application/json" });
          res.end(
            JSON.stringify({
              status: "1",
              message: "OK",
              result: sortedResult,
            }),
          );
        } else {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(
            JSON.stringify({
              status: "0",
              message: "No transactions found",
              result: [],
            }),
          );
        }
        return;
      }

      if (module === "account" && action === "balance") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            status: "1",
            message: "OK",
            result: balanceWei,
          }),
        );
        return;
      }

      if (module === "account" && action === "txlistinternal") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            status: "0",
            message: "No transactions found",
            result: [],
          }),
        );
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
    throw new Error("Mock etherscan server failed to bind to an IPv4 socket");
  }

  return {
    baseUrl: `http://127.0.0.1:${address.port}/`,
    requestCount: () => requestCount,
    status: () => ({
      baseUrl: `http://127.0.0.1:${address.port}/`,
      requestCount,
      responseDelayMs: delayMs,
      transactionCount: txCount,
      dynamicAddresses: serveDynamicAddresses,
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
