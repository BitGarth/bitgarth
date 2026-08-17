import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  TEST_ETH_ADDRESS,
  assertNoBrowserDiagnostics,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

const MOCK_CENTRAL_URL = "http://127.0.0.1:8082";
const PAYMENT_MANAGEMENT_SECRET = "5FuYMBR_MhwubKAJQeNMrUH0JD3PvFuyt3sfFh0ezLw";
const PAYMENT_ORDER_ID = "01JQABCDEF000000000000000E";
const TRANSFER_TOKEN_ID = "01JQABCDEF000000000000000F";

async function resetMockCentral(request) {
  const response = await request.post(`${MOCK_CENTRAL_URL}/__mock/reset`);
  expect(response.ok(), "mock central reset").toBeTruthy();
}

async function setMockCentralScenario(request, scenario) {
  const response = await request.post(`${MOCK_CENTRAL_URL}/__mock/scenario`, {
    data: scenario,
  });
  expect(response.ok(), `mock central scenario ${JSON.stringify(scenario)}`).toBeTruthy();
}

async function installAtlosStub(page) {
  await page.addInitScript(() => {
    const stub = {
      Pay(options) {
        window.__atlosCalls = window.__atlosCalls || [];
        window.__atlosCalls.push({
          merchantId: options.merchantId,
          orderId: options.orderId,
          orderAmount: options.orderAmount,
          orderCurrency: options.orderCurrency,
        });
        window.__atlosCompleted = () => options.onCompleted?.();
        window.__atlosCanceled = () => options.onCanceled?.();
      },
    };
    Object.defineProperty(window, "atlos", {
      value: stub,
      writable: false,
      configurable: true,
    });
  });
  await page.route("https://atlos.io/packages/app/atlos.js", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/javascript",
      body: "// stubbed by test",
    });
  });
}

function subscriptionTransferPayload() {
  return {
    version: 4,
    exported_at: "2026-04-25T12:00:00Z",
    bitgarth_version: "0.1.0",
    wallets: [],
    settings: null,
    api_keys: [
      {
        provider: "etherscan",
        api_key: "e2e-etherscan-api-key",
      },
    ],
    subscription_transfer: {
      exported_at: "2026-04-25T12:00:00Z",
      management_secret: PAYMENT_MANAGEMENT_SECRET,
      active_token: null,
      token_id: null,
      subscription_subject_id: null,
      subscription_valid_until: null,
      token_expires_at: null,
      token_issued_at: null,
      orders: [
        {
          order_id: PAYMENT_ORDER_ID,
          product_tier: "premium",
          order_amount_minor_units: 999,
          order_currency: "USD",
          order_display_scale: 2,
          status: "paid",
          paid_at: "2026-04-25T12:00:00Z",
        },
      ],
    },
  };
}

function rawWalletDataPayload() {
  return {
    version: 4,
    exported_at: "2026-04-25T12:00:00Z",
    bitgarth_version: "0.1.0",
    wallets: [],
    settings: null,
    api_keys: [
      {
        provider: "etherscan",
        api_key: "raw-json-etherscan-api-key",
      },
    ],
  };
}

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function storedZipBuffer(fileName, contents) {
  const name = Buffer.from(fileName, "utf8");
  const data = Buffer.from(contents, "utf8");
  const checksum = crc32(data);

  const local = Buffer.alloc(30 + name.length);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4);
  local.writeUInt16LE(0x0800, 6);
  local.writeUInt16LE(0, 8);
  local.writeUInt16LE(0, 10);
  local.writeUInt16LE(0, 12);
  local.writeUInt32LE(checksum, 14);
  local.writeUInt32LE(data.length, 18);
  local.writeUInt32LE(data.length, 22);
  local.writeUInt16LE(name.length, 26);
  local.writeUInt16LE(0, 28);
  name.copy(local, 30);

  const centralOffset = local.length + data.length;
  const central = Buffer.alloc(46 + name.length);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt16LE(20, 4);
  central.writeUInt16LE(20, 6);
  central.writeUInt16LE(0x0800, 8);
  central.writeUInt16LE(0, 10);
  central.writeUInt16LE(0, 12);
  central.writeUInt16LE(0, 14);
  central.writeUInt32LE(checksum, 16);
  central.writeUInt32LE(data.length, 20);
  central.writeUInt32LE(data.length, 24);
  central.writeUInt16LE(name.length, 28);
  central.writeUInt16LE(0, 30);
  central.writeUInt16LE(0, 32);
  central.writeUInt16LE(0, 34);
  central.writeUInt16LE(0, 36);
  central.writeUInt32LE(0, 38);
  central.writeUInt32LE(0, 42);
  name.copy(central, 46);

  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4);
  eocd.writeUInt16LE(0, 6);
  eocd.writeUInt16LE(1, 8);
  eocd.writeUInt16LE(1, 10);
  eocd.writeUInt32LE(central.length, 12);
  eocd.writeUInt32LE(centralOffset, 16);
  eocd.writeUInt16LE(0, 20);

  return Buffer.concat([local, data, central, eocd]);
}

async function attachImportFile(page, file) {
  // page.goto fully reloads the app, so the file input is present in the SSR'd
  // DOM before Dioxus has attached its change-event listener. A change event
  // dispatched during that window is dropped and the file is silently ignored.
  // Retry the attach until the app registers the selection — the "Restore from
  // backup" button only enables once the file has been read and described.
  const input = page.getByTestId("wallet-data-import-file");
  const importButton = page.getByTestId("wallet-data-import-button");
  await expect(async () => {
    await input.setInputFiles(file);
    await expect(importButton).toBeEnabled({ timeout: 2_000 });
  }).toPass({ timeout: 15_000 });
}

async function attachPremiumTransferFile(page) {
  await attachImportFile(page, {
    name: "subscription-wallet-data.zip",
    mimeType: "application/zip",
    buffer: storedZipBuffer("wallet-data.json", JSON.stringify(subscriptionTransferPayload())),
  });
}

async function attachRawWalletDataJsonFile(page) {
  await attachImportFile(page, {
    name: "wallet-data.json",
    mimeType: "application/json",
    buffer: Buffer.from(JSON.stringify(rawWalletDataPayload()), "utf8"),
  });
}

async function addEthereumWalletForExport(page) {
  const response = await page.request.post("/_app/user/wallets/ethereum/add", {
    data: {
      request: {
        address: TEST_ETH_ADDRESS,
        network: "mainnet",
        wallet_label: "E2E Export Wallet",
      },
    },
  });
  expect(response.ok(), "add wallet for export").toBeTruthy();
}

async function activatePremiumViaMockPayment(page, request) {
  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");
  await page.getByTestId("payments-buy-btn-premium").click();

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "pending", { timeout: 10_000 });

  await setMockCentralScenario(request, {
    orderStatus: "paid",
    paidTokenPayload: {
      premium_access_token: "test-token",
      token_id: TRANSFER_TOKEN_ID,
    },
  });
  await page
    .getByTestId("payments-check-now-btn")
    .click({ timeout: 10_000 })
    .catch(async (error) => {
      if ((await card.getAttribute("data-status")) !== "active") {
        throw error;
      }
    });
  await expect(card).toHaveAttribute("data-status", "active", { timeout: 10_000 });
}

test.beforeEach(async ({ page, request }) => {
  await resetMockCentral(request);
  await installAtlosStub(page);
});

test("wallet-data export exposes explicit subscription transfer opt-in and warning", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await activatePremiumViaMockPayment(page, request);
  await addEthereumWalletForExport(page);
  await page.goto("/exports/wallet-data");

  const checkbox = page.getByTestId("wallet-data-subscription-transfer-checkbox");
  await expect(checkbox).toBeEnabled();
  await checkbox.check();
  await expect(page.getByTestId("wallet-data-subscription-transfer-warning")).toContainText(
    "subscription transfer secret",
  );
  await page.getByTestId("wallet-data-export-password").fill("weak");
  await page.getByTestId("wallet-data-export-confirm-password").fill("weak");
  await expect(page.getByTestId("wallet-data-export-password-guidance")).toContainText(
    "long random password",
  );

  await page.getByTestId("wallet-data-export-button").click();
  await expect(page.getByText("Subscription transfer data was included.")).toBeVisible({
    timeout: 10_000,
  });

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("wallet-data import can move subscription to the current local user", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/exports/wallet-data");
  await attachPremiumTransferFile(page);
  await expect(page.getByText("Missing providers will be imported")).toBeVisible({
    timeout: 10_000,
  });
  await expect(page.getByText("This backup contains subscription transfer data.")).toBeVisible();
  await page.getByTestId("wallet-data-import-button").click();

  await expect(page.getByText("Subscription transfer data found in this backup.")).toBeVisible({
    timeout: 10_000,
  });
  await expect(page.getByRole("button", { name: "Move subscription here" })).toBeVisible();
  await page.getByTestId("wallet-data-subscription-transfer-confirm-button").click();

  await expect(page.getByTestId("wallet-data-subscription-transfer-result")).toContainText(
    "Subscription moved",
    { timeout: 10_000 },
  );
  await expect(page.getByTestId("wallet-data-subscription-transfer-confirm-button")).toHaveCount(0);

  await page.goto("/payments");
  await expect(page.getByTestId("payments-card")).toHaveAttribute("data-status", "active", {
    timeout: 10_000,
  });

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("wallet-data subscription transfer service failure keeps retry available", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await setMockCentralScenario(request, { transferOutcome: "service_unavailable" });
  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/exports/wallet-data");
  await attachPremiumTransferFile(page);
  await expect(page.getByText("Missing providers will be imported")).toBeVisible({
    timeout: 10_000,
  });
  await page.getByTestId("wallet-data-import-button").click();

  await expect(page.getByText("Subscription transfer data found in this backup.")).toBeVisible({
    timeout: 10_000,
  });
  await page.getByTestId("wallet-data-subscription-transfer-confirm-button").click();

  await expect(page.getByTestId("wallet-data-subscription-transfer-result")).toContainText(
    "could not reach the payment service",
    { timeout: 10_000 },
  );
  await expect(page.getByTestId("wallet-data-subscription-transfer-confirm-button")).toBeVisible();

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("wallet-data import accepts raw json backup files", async ({ page }, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/exports/wallet-data");
  await attachRawWalletDataJsonFile(page);
  await expect(page.getByText("Missing providers will be imported")).toBeVisible({
    timeout: 10_000,
  });

  await page.getByTestId("wallet-data-import-button").click();

  await expect(page.getByText("API keys imported")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".import-summary").getByText("1", { exact: true })).toBeVisible();

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
