#!/usr/bin/env node

// Standalone screenshot capture tool using Playwright as a library.
// Not a test spec — no test runner, no assertions, just browser automation
// and screenshot capture.
//
// Captures every page at the desktop and mobile layout tiers. The Botanical
// Ledger design system is single-palette, so there is no theme axis to
// enumerate.

import { chromium } from "@playwright/test";
import { parseArgs } from "node:util";
import { mkdir } from "node:fs/promises";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { startMockServers } from "../helpers/mock-servers.mjs";
import { startMockCentralServer } from "../helpers/mock-central.mjs";
import { startMockCoingeckoServer } from "../helpers/mock-coingecko.mjs";
import {
  claimRunDirectory,
  ensureRunSubdirs,
  writeRunMetadata,
} from "../helpers/run-artifacts.mjs";
import {
  registerViaUiAndExpectAuthenticated,
  configureMockServers,
  TEST_BTC_ADDRESS,
  TEST_ETH_ADDRESS,
} from "../helpers/auth.mjs";
import { HOLDINGS_REPORT_OTHER_BTC_ADDRESS, pages } from "./pages.mjs";
import {
  fetchProductionProductOptions,
  seedMockCentralProductOptions,
} from "./product-options.mjs";

// Test signing public key matching the test private key used by the mock
// Central server (same value playwright.config.mjs passes for E2E).
const PAYMENT_SIGNING_PUBLIC_KEY_B64 =
  "O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik";

// Realistic BTC holding for the showcase wallet: one confirmed receive in
// 2023, then a confirmed send of a smaller amount in 2025 with a small fee.
// The send spends the receive's output and returns change to TEST_BTC_ADDRESS.
// Net on-chain balance = 0.30407540 BTC. Served by the mock mempool as a
// per-address override so the shared fixture is left untouched.
//
//   receive  +0.42910000 BTC (42,910,000 sats), 2023-08-17
//   send     -0.12500000 BTC out + 0.30407540 change back, fee 2,460 sats, 2025-09-22
const SCREENSHOT_BTC_RECEIVE_TXID =
  "c0ffee00000000000000000000000000000000000000000000000000000000a1";
const SCREENSHOT_BTC_TRANSACTIONS = [
  {
    txid: SCREENSHOT_BTC_RECEIVE_TXID,
    vin: [
      {
        txid: "1111111111111111111111111111111111111111111111111111111111111111",
        vout: 0,
        prevout: {
          scriptpubkey_address: "bc1q40s4njz45yfr3q0maqh2pjzk5fxfnpmzqqrz09",
          value: 42_915_400,
        },
      },
    ],
    vout: [
      {
        scriptpubkey: "00144be229a5c18db975e60f9d932eb0cb3bda4d0b2c",
        scriptpubkey_address: TEST_BTC_ADDRESS,
        value: 42_910_000,
      },
    ],
    fee: 5_400,
    status: {
      confirmed: true,
      block_height: 886_000,
      block_hash:
        "00000000000000000000aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff0",
      block_time: Math.floor(Date.UTC(2023, 7, 17, 12, 0, 0) / 1000),
    },
  },
  {
    txid: "c0ffee00000000000000000000000000000000000000000000000000000000a2",
    vin: [
      {
        txid: SCREENSHOT_BTC_RECEIVE_TXID,
        vout: 0,
        prevout: {
          scriptpubkey_address: TEST_BTC_ADDRESS,
          value: 42_910_000,
        },
      },
    ],
    vout: [
      {
        scriptpubkey: "0014aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        scriptpubkey_address: "bc1q4w5h7l2c0xq2m7v9n8s3k6f4d5g8h0j2p3r5t7",
        value: 12_500_000,
      },
      {
        scriptpubkey: "00144be229a5c18db975e60f9d932eb0cb3bda4d0b2c",
        scriptpubkey_address: TEST_BTC_ADDRESS,
        value: 30_407_540,
      },
    ],
    fee: 2_460,
    status: {
      confirmed: true,
      block_height: 910_000,
      block_hash:
        "00000000000000000000bbbb2222cccc3333dddd4444eeee5555ffff6666aaaa1",
      block_time: Math.floor(Date.UTC(2025, 8, 22, 12, 0, 0) / 1000),
    },
  },
];

const OTHER_WALLET_BTC_RECEIVE_TXID =
  "d0ffee00000000000000000000000000000000000000000000000000000000b1";
const OTHER_WALLET_BTC_TRANSACTIONS = [
  {
    txid: OTHER_WALLET_BTC_RECEIVE_TXID,
    vin: [
      {
        txid: "2222222222222222222222222222222222222222222222222222222222222222",
        vout: 0,
        prevout: {
          scriptpubkey_address: "1BoatSLRHtKNngkdXEeobR76b53LETtpyT",
          value: 20_003_100,
        },
      },
    ],
    vout: [
      {
        scriptpubkey: "76a91462e907b15cbf27d5425399ebf6f0fb50ebb88f1888ac",
        scriptpubkey_address: HOLDINGS_REPORT_OTHER_BTC_ADDRESS,
        value: 20_000_000,
      },
    ],
    fee: 3_100,
    status: {
      confirmed: true,
      block_height: 882_000,
      block_hash:
        "00000000000000000000cccc3333dddd4444eeee5555ffff6666aaaa7777bbbb2",
      block_time: Math.floor(Date.UTC(2023, 6, 10, 12, 0, 0) / 1000),
    },
  },
];

// Showcase ETH account: one confirmed 2023 receive and one 2025 send. The
// final live balance matches the transfer value minus the gas fee:
// 1.847259 - 0.65 - (21000 * 12 gwei) = 1.197007 ETH.
const SCREENSHOT_ETH_BALANCE_WEI = "1197007000000000000";
const SCREENSHOT_ETH_RECEIVE_VALUE_WEI = "1847259000000000000";
const SCREENSHOT_ETH_SEND_VALUE_WEI = "650000000000000000";
const SCREENSHOT_ETH_TRANSACTIONS = [
  {
    blockNumber: "21650000",
    timeStamp: String(Math.floor(Date.UTC(2023, 8, 6, 12, 0, 0) / 1000)),
    hash: "0xabc0000000000000000000000000000000000000000000000000000000000001",
    from: "0x1111111111111111111111111111111111111111",
    to: TEST_ETH_ADDRESS,
    value: SCREENSHOT_ETH_RECEIVE_VALUE_WEI,
    gasPrice: "12000000000",
    gasUsed: "21000",
    isError: "0",
    txreceipt_status: "1",
    nonce: "0",
  },
  {
    blockNumber: "23150000",
    timeStamp: String(Math.floor(Date.UTC(2025, 7, 12, 12, 0, 0) / 1000)),
    hash: "0xabc0000000000000000000000000000000000000000000000000000000000002",
    from: TEST_ETH_ADDRESS,
    to: "0x2222222222222222222222222222222222222222",
    value: SCREENSHOT_ETH_SEND_VALUE_WEI,
    gasPrice: "12000000000",
    gasUsed: "21000",
    isError: "0",
    txreceipt_status: "1",
    nonce: "1",
  },
];

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_OUTPUT_DIR = path.resolve("test-results", "screenshots");
const DEFAULT_STABILIZE_MS = 200;

// Viewport sizes aligned to the primary desktop and mobile layouts.
const LAYOUT_TIERS = [
  { name: "desktop", viewport: { width: 1280, height: 720 } },
  { name: "mobile", viewport: { width: 390, height: 844 } },
];

const { values: args } = parseArgs({
  options: {
    "base-url": { type: "string" },
    "self-contained": { type: "boolean", default: false },
    name: { type: "string" },
    "output-dir": { type: "string", default: DEFAULT_OUTPUT_DIR },
    "stabilize-ms": { type: "string", default: String(DEFAULT_STABILIZE_MS) },
  },
});

const isSelfContained = !args["base-url"];
let baseUrl = args["base-url"] ?? null;
const filterName = args.name;
const outputRootDir = path.resolve(args["output-dir"]);
const stabilizeMs = Number(args["stabilize-ms"]);
const {
  runDir: runOutputDir,
  timestampUtc,
} = claimRunDirectory({
  testType: "screenshots",
  rootDir: outputRootDir,
});
const {
  artifactsDir,
  logsDir,
  dataDir,
} = await ensureRunSubdirs(runOutputDir, {
  artifacts: true,
  logs: true,
  data: isSelfContained,
});

// Build output dir map: tier -> path (screenshots live under runDir/screenshots/)
const screenshotsDir = path.join(artifactsDir, "screenshots");
const outputDirMap = {};
for (const tier of LAYOUT_TIERS) {
  outputDirMap[tier.name] = path.join(screenshotsDir, tier.name);
}

const SERVER_HOST = "127.0.0.1";
const SERVER_PORT = 8081;
const SERVER_BINARY = "./target/dx/bitgarth-app/release/web/server";
const HEALTH_TIMEOUT_MS = 120_000;
const HEALTH_POLL_MS = 500;

async function startAppServer(extraEnv = {}) {
  const projectDir = dataDir;
  const serverUrl = `http://${SERVER_HOST}:${SERVER_PORT}`;

  await mkdir(projectDir, { recursive: true });
  const logPath = path.join(logsDir, "server.log");
  const logStream = fs.createWriteStream(logPath);

  console.log("Starting app server...");
  console.log(`  Binary:     ${SERVER_BINARY}`);
  console.log(`  Project dir: ${projectDir}`);
  console.log(`  Log:         ${logPath}`);

  const child = spawn(SERVER_BINARY, [], {
    env: {
      ...process.env,
      IP: SERVER_HOST,
      PORT: String(SERVER_PORT),
      RUST_LOG: "warn",
      BGTRACES: "fs",
      BITGARTH_PROJECT_DIR: projectDir,
      RUST_BACKTRACE: "1",
      // Suppress any ambient operator notice from the developer's shell/.env so
      // showcase screenshots are clean. Empty value renders no banner.
      BITGARTH_INSTANCE_NOTICE_INFO: "",
      ...extraEnv,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });

  child.stdout.pipe(logStream);
  child.stderr.pipe(logStream);

  let exited = false;
  child.on("exit", (code) => {
    exited = true;
    if (code !== null && code !== 0) {
      console.error(`  App server exited with code ${code}`);
    }
  });

  const healthUrl = `${serverUrl}/health`;
  const start = Date.now();

  while (Date.now() - start < HEALTH_TIMEOUT_MS) {
    if (exited) {
      throw new Error("App server exited before becoming healthy — check server.log");
    }
    try {
      const response = await fetch(healthUrl);
      if (response.ok) {
        console.log(`  Server ready at ${serverUrl}`);
        break;
      }
    } catch {
      // Not ready yet
    }
    await new Promise((resolve) => setTimeout(resolve, HEALTH_POLL_MS));
  }

  if (Date.now() - start >= HEALTH_TIMEOUT_MS) {
    child.kill();
    throw new Error(`Server failed to start within ${HEALTH_TIMEOUT_MS}ms — check server.log`);
  }

  return {
    url: serverUrl,
    stop: async () => {
      if (!exited) {
        child.kill();
        await new Promise((resolve) => {
          child.on("exit", resolve);
          setTimeout(resolve, 5_000);
        });
      }
      logStream.end();
      console.log("  App server stopped.");
    },
  };
}

const targetPages = filterName
  ? pages.filter((p) => p.name === filterName)
  : pages;

if (targetPages.length === 0) {
  const available = pages.map((p) => p.name).join(", ");
  console.error(`No page found with name: ${filterName}`);
  console.error(`Available pages: ${available}`);
  process.exit(1);
}

async function capturePage(page, pageDef, stabilize) {
  await page.goto(pageDef.path, { waitUntil: "load" });
  await page.waitForSelector(pageDef.waitFor, { state: "visible", timeout: 15_000 });
  await page.waitForTimeout(stabilize);
}

async function prepareFullPageScreenshot(page, tier) {
  if (tier.name !== "mobile") {
    return;
  }

  const isTallerThanViewport = await page.evaluate(
    () => document.documentElement.scrollHeight > window.innerHeight,
  );
  if (!isTallerThanViewport) {
    return;
  }

  await page.addStyleTag({
    content: ".mobile-bottom-bar { display: none !important; }",
  });
}

async function main() {
  await writeRunMetadata(runOutputDir, {
    test_type: "screenshots",
    timestamp_utc: timestampUtc,
    run_dir: path.relative(process.cwd(), runOutputDir),
    project_dir: isSelfContained ? path.relative(process.cwd(), dataDir) : null,
    command: "node tests/e2e/screenshots/capture.mjs",
    exit_code: null,
    git_commit: null,
    git_dirty: null,
    notes: isSelfContained ? "self-contained screenshot run" : "external-base-url screenshot run",
  });

  for (const tier of LAYOUT_TIERS) {
    await mkdir(outputDirMap[tier.name], { recursive: true });
  }

  const results = [];
  let mockServers;
  let mockCentral;
  let mockCoingecko;
  let appServer;

  try {
    if (isSelfContained) {
      // Central + CoinGecko base URLs are env-only (not per-user settings), so
      // their mocks must be up before the app server starts.
      mockCentral = await startMockCentralServer({ port: 0 });
      mockCoingecko = await startMockCoingeckoServer({ port: 0 });
      console.log(`  Central:   ${mockCentral.baseUrl}`);
      console.log(`  CoinGecko: ${mockCoingecko.baseUrl}`);

      console.log("Fetching production product-options...");
      const productionProductOptions = await fetchProductionProductOptions();
      await seedMockCentralProductOptions(
        {
          async post(path, options) {
            const response = await fetch(path, {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify(options.data),
            });
            return {
              ok() {
                return response.ok;
              },
              status() {
                return response.status;
              },
            };
          },
        },
        mockCentral.baseUrl,
        productionProductOptions,
      );
      console.log(
        `  Product options: ${productionProductOptions.tiers.length} production tiers`,
      );

      appServer = await startAppServer({
        BITGARTH_CENTRAL_BASE_URL: mockCentral.baseUrl,
        BITGARTH_COINGECKO_BASE_URL: mockCoingecko.baseUrl,
        // Deliberately no BITGARTH_CHANNEL: an unknown channel keeps the
        // software-update indicator dormant so the showcase images stay clean,
        // while payment product-options still resolve from mock Central.
        BITGARTH_PAYMENT_SIGNING_PUBLIC_KEY_B64: PAYMENT_SIGNING_PUBLIC_KEY_B64,
      });
      baseUrl = appServer.url;
    }

    console.log("Starting mock servers...");
    mockServers = await startMockServers({
      mempoolAddressData: {
        [TEST_BTC_ADDRESS]: {
          // funded = receive 42,910,000 + change 30,407,540 = 73,317,540
          // spent  = receive output spent by the send = 42,910,000
          // balance = 30,407,540 sats (0.30407540 BTC)
          stats: {
            funded_txo_sum: 73_317_540,
            spent_txo_sum: 42_910_000,
            tx_count: SCREENSHOT_BTC_TRANSACTIONS.length,
          },
          txs: SCREENSHOT_BTC_TRANSACTIONS,
        },
        [HOLDINGS_REPORT_OTHER_BTC_ADDRESS]: {
          stats: {
            funded_txo_sum: 20_000_000,
            spent_txo_sum: 0,
            tx_count: OTHER_WALLET_BTC_TRANSACTIONS.length,
          },
          txs: OTHER_WALLET_BTC_TRANSACTIONS,
        },
      },
      etherscanNativeBalanceWei: SCREENSHOT_ETH_BALANCE_WEI,
      etherscanKnownAddressTransactions: SCREENSHOT_ETH_TRANSACTIONS,
    });
    console.log(`  Etherscan: ${mockServers.etherscan.baseUrl}`);
    console.log(`  Mempool:   ${mockServers.mempool.baseUrl}`);

    const browser = await chromium.launch();

    try {
      const unauthPages = targetPages.filter((p) => !p.auth);
      const authPages = targetPages.filter((p) => p.auth);

      // Unauthenticated pages — one context per (page, tier)
      for (const tier of LAYOUT_TIERS) {
        for (const pageDef of unauthPages) {
          const context = await browser.newContext({
            baseURL: baseUrl,
            viewport: tier.viewport,
          });
          const page = await context.newPage();

          try {
            console.log(`Capturing ${pageDef.name} (${tier.name})...`);
            const outputPath = path.join(
              outputDirMap[tier.name],
              `${pageDef.name}.png`,
            );
            await capturePage(page, pageDef, stabilizeMs);
            if (pageDef.interact) {
              await pageDef.interact(page);
            }
            await prepareFullPageScreenshot(page, tier);
            await page.waitForTimeout(stabilizeMs);
            await page.screenshot({ path: outputPath, fullPage: true });
            results.push({
              name: pageDef.name,
              tier: tier.name,
              status: "ok",
              path: outputPath,
            });
            console.log(`  saved ${outputPath}`);
          } catch (err) {
            results.push({
              name: pageDef.name,
              tier: tier.name,
              status: "error",
              error: err.message,
            });
            console.error(
              `  FAILED ${pageDef.name} (${tier.name}): ${err.message}`,
            );
          } finally {
            await context.close();
          }
        }
      }

      if (authPages.length > 0) {
        // Authenticated pages — register once, resize for tiers
        const desktopTier = LAYOUT_TIERS[0];
        const context = await browser.newContext({
          baseURL: baseUrl,
          viewport: desktopTier.viewport,
        });
        const page = await context.newPage();

        try {
          console.log("Registering fresh user...");
          await registerViaUiAndExpectAuthenticated(page, undefined, {
            useGeneratedUsername: true,
          });
          console.log("  User registered.");

          await configureMockServers(page.request, mockServers);
          console.log("  Mock servers configured.");

          // Share the mock-central handle so showcase setup can configure the
          // paid scenario. Null under --base-url (external) runs.
          const setupContext = { mockCentral: mockCentral ?? null };

          // Process each page in order: run its setup, then capture at all tiers.
          // Order matters — wallets-empty must be captured before wallets-populated's
          // setup adds data, and account-transactions depends on wallets-populated's
          // setup context.
          for (const pageDef of authPages) {
            let effectivePath = pageDef.path;
            let setupFailed = false;

            if (pageDef.setup) {
              try {
                console.log(`  Running setup for ${pageDef.name}...`);
                const result = await pageDef.setup(
                  page.request,
                  mockServers,
                  setupContext,
                );
                if (result?.path) {
                  effectivePath = result.path;
                }
              } catch (err) {
                setupFailed = true;
                console.error(`  SETUP FAILED ${pageDef.name}: ${err.message}`);
                for (const tier of LAYOUT_TIERS) {
                  results.push({
                    name: pageDef.name,
                    tier: tier.name,
                    status: "error",
                    error: `Setup failed: ${err.message}`,
                  });
                }
              }
            }

            if (setupFailed) {
              continue;
            }

            const effectiveDef = { ...pageDef, path: effectivePath };

            for (const tier of LAYOUT_TIERS) {
              try {
                await page.setViewportSize(tier.viewport);
                const outputPath = path.join(
                  outputDirMap[tier.name],
                  `${pageDef.name}.png`,
                );

                console.log(
                  `Capturing ${pageDef.name} (${tier.name})...`,
                );
                await capturePage(page, effectiveDef, stabilizeMs);
                if (effectiveDef.interact) {
                  await effectiveDef.interact(page);
                }
                await prepareFullPageScreenshot(page, tier);
                await page.waitForTimeout(stabilizeMs);
                await page.screenshot({ path: outputPath, fullPage: true });
                results.push({
                  name: pageDef.name,
                  tier: tier.name,
                  status: "ok",
                  path: outputPath,
                });
                console.log(`  saved ${outputPath}`);
              } catch (err) {
                results.push({
                  name: pageDef.name,
                  tier: tier.name,
                  status: "error",
                  error: err.message,
                });
                console.error(
                  `  FAILED ${pageDef.name} (${tier.name}): ${err.message}`,
                );
              }
            }
          }
        } catch (err) {
          console.error(`Auth setup failed: ${err.message}`);
          for (const pageDef of authPages) {
            for (const tier of LAYOUT_TIERS) {
              if (
                !results.some(
                  (r) =>
                    r.name === pageDef.name && r.tier === tier.name,
                )
              ) {
                results.push({
                  name: pageDef.name,
                  tier: tier.name,
                  status: "skipped",
                  error: "Auth setup failed",
                });
              }
            }
          }
        } finally {
          await context.close();
        }
      }
    } finally {
      await browser.close();
    }
  } finally {
    if (mockServers) {
      await mockServers.close();
    }
    if (appServer) {
      await appServer.stop();
    }
    if (mockCentral) {
      await mockCentral.close();
    }
    if (mockCoingecko) {
      await mockCoingecko.close();
    }
  }

  // Summary
  console.log("\n--- Summary ---");
  const ok = results.filter((r) => r.status === "ok");
  const failed = results.filter((r) => r.status !== "ok");

  console.log(`Run output: ${runOutputDir}`);

  for (const r of ok) {
    console.log(`  OK     ${r.tier}/${r.name} -> ${r.path}`);
  }
  for (const r of failed) {
    console.log(`  FAILED ${r.tier}/${r.name}: ${r.error}`);
  }

  console.log(`\n${ok.length} captured, ${failed.length} failed.`);

  if (failed.length > 0) {
    process.exit(1);
  }
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
