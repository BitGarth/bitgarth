import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  addAndSyncLimitedBitcoinAccount,
  assertNoBrowserDiagnostics,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

const MOCK_CENTRAL_URL = "http://127.0.0.1:8082";

test("unavailable Bitcoin history does not invent report price requirements", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START unavailable-bitcoin-prices");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  const { wallet_id: walletId } = await addAndSyncLimitedBitcoinAccount(
    page,
    mockServers,
    "E2E Unavailable Bitcoin Prices",
  );

  await page.goto(`/wallets/${walletId}`);
  const row = page.locator(".wr-row").filter({ hasText: "Limited Bitcoin" });
  await expect(row.locator(".wr-crypto-value")).toHaveText([
    "Not available",
    "Not available",
  ]);
  await expect(row.locator(".wr-fiat-value")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Set price" })).toHaveCount(0);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END unavailable-bitcoin-prices");
});

async function activatePremiumForPriceOverrides(request) {
  const resetResponse = await request.post(`${MOCK_CENTRAL_URL}/__mock/reset`);
  expect(resetResponse.ok()).toBeTruthy();

  const scenarioResponse = await request.post(`${MOCK_CENTRAL_URL}/__mock/scenario`, {
    data: {
      orderStatus: "paid",
      paidTokenPayload: {
        premium_access_token: "test-token",
      },
    },
  });
  expect(scenarioResponse.ok()).toBeTruthy();

  const startResponse = await request.post("/_app/user/payments/premium/start", {
    data: {
      product_option_id: "premium_12_months_usd",
    },
  });
  expect(startResponse.ok()).toBeTruthy();
  const startPayload = await startResponse.json();
  const orderId = startPayload?.central_order_id;
  expect(orderId).toBeTruthy();

  const pollResponse = await request.post("/_app/user/payments/premium/poll", {
    data: {
      order_id: orderId,
    },
  });
  expect(pollResponse.ok()).toBeTruthy();
}

test("wallet report Prices section saves a boundary price and persists across reload", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");

  await registerViaUiAndExpectAuthenticated(page);
  await activatePremiumForPriceOverrides(page.request);

  const addManualAssetResponse = await page.request.post(
    "/_app/user/wallets/manual-assets/add",
    {
      data: {
        request: {
          wallet_label: "E2E Prices Section",
          asset_instance_id: {
            asset_id: "monero",
            network_id: "monero-mainnet",
          },
        },
      },
    },
  );
  expect(addManualAssetResponse.ok()).toBeTruthy();
  const addManualAssetPayload = await addManualAssetResponse.json();
  const walletId = addManualAssetPayload.wallet_id;
  const accountId = addManualAssetPayload.account_id;
  expect(walletId).toBeTruthy();
  expect(accountId).toBeTruthy();

  const addAssertionResponse = await page.request.post(
    "/_app/user/manual-asset-assertions/add",
    {
      data: {
        request: {
          account_id: accountId,
          asserted_on: "2026-01-01",
          balance: "2.5",
          note: null,
        },
      },
    },
  );
  expect(addAssertionResponse.ok()).toBeTruthy();

  const reportPage = await page.context().newPage();
  const diagnostics = await attachBrowserDiagnostics(reportPage, testInfo);

  await reportPage.goto(`/wallets/${walletId}`);
  await expect(
    reportPage.getByRole("heading", { name: /^E2E Prices Section\b/ }),
  ).toBeVisible();

  // Prices section should appear and auto-expand when boundary prices are missing.
  const pricesStrip = reportPage.getByRole("button", { name: /§ Prices/ });
  await expect(pricesStrip).toBeVisible({ timeout: 15_000 });
  await expect(pricesStrip).toHaveAttribute("aria-expanded", "true");
  await expect(pricesStrip).toContainText("missing");

  // Click the first "Set price" button (opening boundary for the ETH subject).
  const setPriceButtons = reportPage.getByRole("button", { name: "Set price" });
  await expect(setPriceButtons.first()).toBeVisible();
  await setPriceButtons.first().click();

  const priceInput = reportPage
    .getByRole("textbox", { name: /Price per unit/ })
    .first();
  await priceInput.fill("2500");
  await reportPage.getByRole("button", { name: "Save" }).first().click();

  // After save, the Prices section should display the saved value as a button.
  await expect(reportPage.locator(".wr-prices-price-display").first()).toContainText(
    "2500",
    { timeout: 10_000 },
  );

  // Reload and confirm the saved override persists in the encrypted user DB.
  await reportPage.reload();
  await expect(
    reportPage.getByRole("heading", { name: /^E2E Prices Section\b/ }),
  ).toBeVisible();
  // The persisted override now resolves the boundary, so the Prices section
  // is no longer "missing" and collapses by default; expand it to read the value.
  const pricesStripAfterReload = reportPage.getByRole("button", { name: /§ Prices/ });
  await expect(pricesStripAfterReload).toBeVisible({ timeout: 15_000 });
  if ((await pricesStripAfterReload.getAttribute("aria-expanded")) !== "true") {
    await pricesStripAfterReload.click();
  }
  await expect(reportPage.locator(".wr-prices-price-display").first()).toContainText(
    "2500",
    { timeout: 10_000 },
  );

  assertNoBrowserDiagnostics(diagnostics);
  await reportPage.close();
  await markTestBoundary(testInfo, "END");
});
