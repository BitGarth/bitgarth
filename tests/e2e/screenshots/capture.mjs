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
import path from "node:path";

import { startMockServers } from "../helpers/mock-servers.mjs";
import { startMockCentralServer } from "../helpers/mock-central.mjs";
import { startMockCoingeckoServer } from "../helpers/mock-coingecko.mjs";
import {
  claimRunDirectory,
  ensureRunSubdirs,
  writeRunMetadata,
} from "../helpers/run-artifacts.mjs";
import {
  loginViaUi,
  registerViaUiAndExpectAuthenticated,
  configureMockServers,
  TEST_BTC_ADDRESS,
  TEST_ETH_ADDRESS,
} from "../helpers/auth.mjs";
import { pages } from "./pages.mjs";
import {
  fixtureHash, OTHER_BTC_ADDRESS, OTHER_BTC, SHOWCASE_BTC, SHOWCASE_NOW,
  SHOWCASE_TIP, SHOWCASE_USERNAME, SHOWCASE_ZONE, seedShowcasePrices, seedShowcaseUser,
} from "./showcase.mjs";
import {
  fetchProductionProductOptions,
  seedMockCentralProductOptions,
} from "./product-options.mjs";

// Test signing public key matching the test private key used by the mock
// Central server (same value playwright.config.mjs passes for E2E).
const PAYMENT_SIGNING_PUBLIC_KEY_B64 =
  "O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik";

// Showcase ETH account: one confirmed 2023 receive and one 2025 send. The
// final live balance matches the transfer value minus the gas fee:
// 1.847259 - 0.65 - (21000 * 12 gwei) = 1.197007 ETH.
const SCREENSHOT_ETH_BALANCE_WEI = "1197007000000000000";
const SCREENSHOT_ETH_RECEIVE_VALUE_WEI = "1847259000000000000";
const SCREENSHOT_ETH_SEND_VALUE_WEI = "650000000000000000";
const SCREENSHOT_ETH_TRANSACTIONS = [
  {
    blockNumber: "21650000",
    timeStamp: String(Math.floor(Date.UTC(2023, 8, 6, 9, 38) / 1000)),
    hash: "0x" + fixtureHash("eth/receive"),
    from: "0x" + fixtureHash("eth/deposit-account").slice(-40),
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
    timeStamp: String(Math.floor(Date.UTC(2025, 7, 12, 18, 26) / 1000)),
    hash: "0x" + fixtureHash("eth/send"),
    from: TEST_ETH_ADDRESS,
    to: "0x" + fixtureHash("eth/withdrawal-account").slice(-40),
    value: SCREENSHOT_ETH_SEND_VALUE_WEI,
    gasPrice: "12000000000",
    gasUsed: "21000",
    isError: "0",
    txreceipt_status: "1",
    nonce: "1",
  },
];

const DEFAULT_OUTPUT_DIR = path.resolve("test-results", "screenshots");
const DEFAULT_STABILIZE_MS = 200;

// Viewport sizes aligned to the primary desktop and mobile layouts.
const LAYOUT_TIERS = [
  { name: "desktop", viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 },
  { name: "mobile", viewport: { width: 390, height: 844 }, deviceScaleFactor: 1 },
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
const marketingDir = path.join(screenshotsDir, "marketing");
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

async function newCaptureContext(browser, tier, storageState) {
  const context = await browser.newContext({
    baseURL: baseUrl, viewport: tier.viewport, deviceScaleFactor: tier.deviceScaleFactor,
    timezoneId: SHOWCASE_ZONE, locale: "en-US", storageState,
  });
  await context.clock.setFixedTime(new Date(SHOWCASE_NOW));
  return context;
}

async function capturePage(page, pageDef, tier, outputPath) {
  await page.goto(pageDef.path, { waitUntil: "load" });
  await page.waitForSelector(pageDef.waitFor, { state: "visible", timeout: 15_000 });
  await page.waitForTimeout(stabilizeMs);
  if (pageDef.interact) await pageDef.interact(page);
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(1, 1);
  if (tier.name === "desktop") {
    if (pageDef.frameMarketing) await pageDef.frameMarketing(page);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.mouse.move(1, 1);
    await page.screenshot({
      path: path.join(marketingDir, `${pageDef.name}.png`),
      animations: "disabled", caret: "hide",
    });
    if (pageDef.frameMarketing) {
      await page.goto(pageDef.path);
      await page.waitForSelector(pageDef.waitFor);
    }
  }
  await prepareFullPageScreenshot(page, tier);
  await screenshotFullPage(page, tier, outputPath, stabilizeMs);
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

// Grow the viewport to the page height instead of using `fullPage`, which
// stitches beyond the viewport and leaves fixed layers (sidebar, paper
// texture) ending at the original fold.
async function screenshotFullPage(page, tier, outputPath, stabilizeMs) {
  const height = await page.evaluate(() => document.documentElement.scrollHeight);
  await page.setViewportSize({
    width: tier.viewport.width,
    height: Math.max(tier.viewport.height, height),
  });
  // Park the pointer on the navbar so no row keeps a hover state from an
  // earlier interaction.
  await page.mouse.move(1, 1);
  await page.waitForTimeout(stabilizeMs);
  await page.screenshot({ path: outputPath, animations: "disabled", caret: "hide" });
  await page.setViewportSize(tier.viewport);
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

  await mkdir(marketingDir, { recursive: true });
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
      // Keep the mock's simulated upgrade out of web screenshots.
      mockCentral.setScenario({ latestAppVersions: { channels: { web: { latest: "0.0.0" } } } });
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
        BITGARTH_CHANNEL: "web",
        BITGARTH_PAYMENT_SIGNING_PUBLIC_KEY_B64: PAYMENT_SIGNING_PUBLIC_KEY_B64,
      });
      baseUrl = appServer.url;
    }

    console.log("Starting mock servers...");
    mockServers = await startMockServers({
      mempoolChainTipHeight: SHOWCASE_TIP,
      mempoolAddressData: {
        [TEST_BTC_ADDRESS]: SHOWCASE_BTC,
        [OTHER_BTC_ADDRESS]: OTHER_BTC,
      },
      etherscanNativeBalanceWei: SCREENSHOT_ETH_BALANCE_WEI,
      etherscanKnownAddressTransactions: SCREENSHOT_ETH_TRANSACTIONS,
    });
    console.log(`  Etherscan: ${mockServers.etherscan.baseUrl}`);
    console.log(`  Mempool:   ${mockServers.mempool.baseUrl}`);

    // Repaint the whole surface after viewport changes. Reusing partial tiles
    // can round a handful of gradient pixels differently on tall pages.
    const browser = await chromium.launch({ args: ["--disable-partial-raster", "--num-raster-threads=1"] });

    try {
      const unauthPages = targetPages.filter((p) => !p.auth);
      const authPages = targetPages.filter((p) => p.auth);

      // Unauthenticated pages — one context per (page, tier)
      for (const tier of LAYOUT_TIERS) {
        for (const pageDef of unauthPages) {
          const context = await newCaptureContext(browser, tier);
          const page = await context.newPage();

          try {
            console.log(`Capturing ${pageDef.name} (${tier.name})...`);
            const outputPath = path.join(
              outputDirMap[tier.name],
              `${pageDef.name}.png`,
            );
            await capturePage(page, pageDef, tier, outputPath);
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
        const [desktop, mobile] = LAYOUT_TIERS;
        const context = await newCaptureContext(browser, desktop);
        const page = await context.newPage();
        let mobileContext;

        try {
          if (isSelfContained) {
            await seedShowcaseUser(dataDir);
            seedShowcasePrices(dataDir);
            await page.goto("/login");
            await loginViaUi(page, SHOWCASE_USERNAME);
          } else {
            await registerViaUiAndExpectAuthenticated(page, undefined, { useGeneratedUsername: true });
          }
          mobileContext = await newCaptureContext(browser, mobile, await context.storageState());
          const mobilePage = await mobileContext.newPage();
          await configureMockServers(page.request, mockServers);
          console.log("  Mock servers configured.");

          // Share the mock-central handle so showcase setup can configure the
          // paid scenario. Null under --base-url (external) runs.
          const setupContext = { mockCentral: mockCentral ?? null, seededPrices: isSelfContained };

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
                const outputPath = path.join(
                  outputDirMap[tier.name],
                  `${pageDef.name}.png`,
                );

                console.log(
                  `Capturing ${pageDef.name} (${tier.name})...`,
                );
                await capturePage(tier === desktop ? page : mobilePage, effectiveDef, tier, outputPath);
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
          await mobileContext?.close();
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

  const frames = ok.filter((result) => result.tier === "desktop").length;
  console.log(`\n${ok.length} full-page checks and ${frames} marketing frames captured, ${failed.length} failed.`);

  if (failed.length > 0) {
    process.exit(1);
  }
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
