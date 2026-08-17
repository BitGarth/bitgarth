import { expect } from "@playwright/test";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const etherscanFixtureData = require("../fixtures/etherscan-fixture.json");
const mempoolFixtureData = require("../fixtures/mempool-fixture.json");

export const etherscanFixture = etherscanFixtureData;
export const mempoolFixture = mempoolFixtureData;

export const TEST_ETHERSCAN_API_KEY = etherscanFixtureData.apiKey;
export const TEST_ETH_ADDRESS = etherscanFixtureData.knownAddress;
export const TEST_BTC_ADDRESS = mempoolFixtureData.knownAddress;
export const TEST_UNKNOWN_BTC_ADDRESS = mempoolFixtureData.unknownAddress;
export const TEST_XPUB = mempoolFixtureData.xpubs.xpub;
export const TEST_YPUB = mempoolFixtureData.xpubs.ypub;
export const TEST_ZPUB = mempoolFixtureData.xpubs.zpub;

export const DEFAULT_TEST_PASSWORD = "SecurePass123";
export const TEST_ACCOUNT_XPUB =
  "xpub6C7dm6fpZENX4meEzE4DLTSb4nvYMPiZvJKMnbhGoDTfBMTMsY7eBxmaQq9RpSSKTdFyb5MoE1encwjP99mSHwjJf8JVoo572k9ireBAxyq";

let usernameCounter = 0;

function uniqueE2eUsername(suggestedUsername) {
  usernameCounter += 1;
  const worker = process.env.TEST_WORKER_INDEX ?? "0";
  const suffix = `e2e-${worker}-${Date.now().toString(36)}-${usernameCounter}`;
  const base = suggestedUsername
    .replace(/[^A-Za-z0-9_@.-]/g, "-")
    .slice(0, 64 - suffix.length - 1);

  return `${base}-${suffix}`;
}

export async function waitForGeneratedUsername(page) {
  const usernameElement = page.locator("#username");
  await expect(usernameElement).toBeVisible();

  await page.waitForFunction(() => {
    const element = document.getElementById("username");
    if (!element) return false;
    if (element.tagName.toLowerCase() === "input") {
      return element.value.trim().length > 0;
    }
    return (element.textContent ?? "").trim().length > 0;
  });

  return page.evaluate(() => {
    const element = document.getElementById("username");
    if (!element) return "";
    if (element.tagName.toLowerCase() === "input") {
      return element.value.trim();
    }
    return (element.textContent ?? "").trim();
  });
}

export async function registerViaUiAndExpectAuthenticated(
  page,
  password = DEFAULT_TEST_PASSWORD,
  options = {},
) {
  await page.goto("/register");
  await expect(page.locator("form")).toBeVisible();

  const generatedUsername = await waitForGeneratedUsername(page);
  expect(generatedUsername).not.toBe("");
  const username = options.username
    ?? (options.useGeneratedUsername ? generatedUsername : uniqueE2eUsername(generatedUsername));
  await page.locator("#username").fill(username);

  await page.locator("#password").fill(password);
  await page.locator("#confirm-password").fill(password);
  await page.getByTestId("legal-acknowledgement-checkbox").check();
  await page.locator("form button[type='submit']").click();

  await expect(page.locator(".user-menu-trigger")).toBeVisible({
    timeout: 15_000,
  });

  return { username, password };
}

export async function logoutViaUserMenu(page) {
  await page.locator(".user-menu-trigger").click();
  await page
    .locator(".user-menu-dropdown button.user-menu-item[role='menuitem']")
    .click();
  await expect(page.locator("#username")).toBeVisible({ timeout: 10_000 });
}

export async function loginViaUi(page, username, password = DEFAULT_TEST_PASSWORD) {
  await page.locator("#username").fill(username);
  await page.locator("#password").fill(password);
  await page.locator("form button[type='submit']").click();
  await expect(page.locator(".user-menu-trigger")).toBeVisible({
    timeout: 15_000,
  });
}

export function assertNoBrowserDiagnostics(diagnostics) {
  expect(
    diagnostics.consoleErrors,
    `Console errors:\n${diagnostics.consoleErrors.join("\n")}`,
  ).toEqual([]);
  expect(
    diagnostics.arenaErrors,
    `Dioxus element-arena errors:\n${diagnostics.arenaErrors.join("\n")}`,
  ).toEqual([]);
  expect(
    diagnostics.pageErrors,
    `Page errors:\n${diagnostics.pageErrors.join("\n")}`,
  ).toEqual([]);
  expect(
    diagnostics.requestFailures,
    `Request failures:\n${diagnostics.requestFailures.join("\n")}`,
  ).toEqual([]);
}

export async function getAuthenticatedUserId(requestContext) {
  const response = await requestContext.get("/_app/auth/me");
  expect(response.ok()).toBeTruthy();
  const body = await response.json();
  expect(body?.user?.user_id).toBeTruthy();
  return body.user.user_id;
}

export async function saveMempoolBaseUrl(requestContext, mempoolBaseUrl) {
  const response = await requestContext.post("/_app/user/settings/mempool_base_url", {
    data: { mempool_base_url: mempoolBaseUrl },
  });
  expect(response.ok()).toBeTruthy();
}

export async function saveEtherscanBaseUrl(requestContext, etherscanBaseUrl) {
  const response = await requestContext.post("/_app/user/settings/etherscan_base_url", {
    data: { etherscan_base_url: etherscanBaseUrl },
  });
  expect(response.ok()).toBeTruthy();
}

export async function saveEtherscanApiKey(requestContext, apiKey) {
  const response = await requestContext.post("/_app/user/settings/etherscan_api_key", {
    data: { api_key: apiKey },
  });
  expect(response.ok()).toBeTruthy();
}

export async function configureMockServers(requestContext, mockServers) {
  await saveMempoolBaseUrl(requestContext, mockServers.mempool.baseUrl);
  await saveEtherscanBaseUrl(requestContext, mockServers.etherscan.baseUrl);
  await saveEtherscanApiKey(requestContext, TEST_ETHERSCAN_API_KEY);
}

export async function activatePaidHistoryWithCap(requestContext, maxTransactions) {
  const capabilities = {
    limits: {
      accounts: { total: 10 },
      synced_accounts: 10,
      history: { max_transactions_per_account: maxTransactions },
    },
    features: {
      balance_assertions: true,
      balance_sync: true,
      exchange_rates_current: true,
      exchange_rates_history: true,
      historical_sync: true,
      hledger_export: true,
      price_overrides: true,
      tax_reports: true,
      transaction_history_sync: true,
    },
  };

  expect(
    (
      await requestContext.post("http://127.0.0.1:8082/__mock/reset")
    ).ok(),
  ).toBeTruthy();
  expect(
    (
      await requestContext.post("http://127.0.0.1:8082/__mock/scenario", {
        data: {
          orderStatus: "paid",
          paidTokenPayload: {
            premium_access_token: "test-token",
            tier: "basic",
            capabilities,
          },
        },
      })
    ).ok(),
  ).toBeTruthy();

  const startResponse = await requestContext.post(
    "/_app/user/payments/premium/start",
    { data: { product_option_id: "basic_12_months_usd" } },
  );
  expect(startResponse.ok()).toBeTruthy();
  const { central_order_id: orderId } = await startResponse.json();
  expect(orderId).toBeTruthy();
  expect(
    (
      await requestContext.post("/_app/user/payments/premium/poll", {
        data: { order_id: orderId },
      })
    ).ok(),
  ).toBeTruthy();
  return orderId;
}

export async function addAndSyncLimitedBitcoinAccount(
  page,
  mockServers,
  walletLabel,
) {
  await configureMockServers(page.request, mockServers);
  const orderId = await activatePaidHistoryWithCap(page.request, 1);

  const addResponse = await page.request.post("/_app/user/wallets/bitcoin/add", {
    data: {
      request: {
        address: TEST_BTC_ADDRESS,
        network: "mainnet",
        wallet_id: null,
        wallet_label: walletLabel,
        account_label: "Limited Bitcoin",
      },
    },
  });
  expect(addResponse.ok()).toBeTruthy();
  const added = await addResponse.json();

  const selectResponse = await page.request.post(
    "/_app/user/wallets/account/sync-slot/select",
    { data: { request: { account_id: added.account_id } } },
  );
  expect(selectResponse.ok()).toBeTruthy();

  const syncResponse = await page.request.post("/_app/user/transactions/sync", {
    data: { request: { source: "manual" } },
  });
  expect(syncResponse.ok()).toBeTruthy();

  let history;
  await expect
    .poll(
      async () => {
        const response = await page.request.get(
          `/_app/user/account/${added.account_id}/transactions?pending_page=1&confirmed_page=1`,
        );
        if (!response.ok()) {
          return "unavailable";
        }
        history = await response.json();
        return `${history.bitcoin_history_coverage}:${history.confirmed?.total ?? -1}:${history.pending?.total ?? -1}`;
      },
      { intervals: [250, 500, 1000, 2000], timeout: 30_000 },
    )
    .toBe("limited:2:1");

  return { ...added, history, orderId };
}

export async function validateXpub(requestContext, xpub = TEST_ACCOUNT_XPUB) {
  return requestContext.post("/_app/user/wallets/xpub/validate", {
    data: {
      request: {
        extended_pubkey: xpub,
      },
    },
  });
}
