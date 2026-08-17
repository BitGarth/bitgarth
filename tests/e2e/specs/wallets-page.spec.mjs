import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  TEST_ETH_ADDRESS,
  TEST_UNKNOWN_BTC_ADDRESS,
  addAndSyncLimitedBitcoinAccount,
  assertNoBrowserDiagnostics,
  configureMockServers,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

const MOCK_CENTRAL_URL = "http://127.0.0.1:8082";

async function assertBitcoinBalanceRows(walletCard) {
  const knownRow = walletCard
    .locator(".account-row")
    .filter({ hasText: "Limited Bitcoin" });
  await expect(knownRow.locator(".account-balance")).toContainText("0.00095");
  await expect(knownRow.locator(".account-balance-provisional-label")).toHaveCount(0);

  const unknownRow = walletCard
    .locator(".account-row")
    .filter({ hasText: "Unknown Bitcoin" });
  await expect(unknownRow.locator(".account-balance")).toHaveText("Not available");
  await expect(unknownRow.locator(".account-current-value:not(.is-missing)")).toHaveCount(0);
}

test("wallets keep a known current Bitcoin balance independent of limited history", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START bitcoin-current-balance");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  const { wallet_id: walletId } = await addAndSyncLimitedBitcoinAccount(
    page,
    mockServers,
    "E2E Bitcoin Current Balance",
  );

  const unknownResponse = await page.request.post(
    "/_app/user/wallets/bitcoin/add",
    {
      data: {
        request: {
          address: TEST_UNKNOWN_BTC_ADDRESS,
          network: "mainnet",
          wallet_id: walletId,
          wallet_label: null,
          account_label: "Unknown Bitcoin",
        },
      },
    },
  );
  expect(unknownResponse.ok()).toBeTruthy();

  const walletsResponse = await page.request.get("/_app/user/wallets");
  expect(walletsResponse.ok()).toBeTruthy();
  const walletsBody = await walletsResponse.json();
  const wallet = walletsBody.wallets.find(({ id }) => id === walletId);
  expect(wallet).toBeTruthy();
  const aggregate = wallet.balances.find(({ asset_id }) => asset_id === "bitcoin");
  expect(aggregate?.balance_state).toEqual({ kind: "unknown" });
  expect(aggregate?.current_value).toBeNull();

  await page.goto("/wallets");
  const desktopWalletCard = page
    .locator(".wallet-card")
    .filter({ hasText: "E2E Bitcoin Current Balance" });
  await assertBitcoinBalanceRows(desktopWalletCard);
  await expect(desktopWalletCard.locator(".wallet-value-summary")).toHaveCount(0);

  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  const mobileWalletCard = page
    .locator(".wallet-card")
    .filter({ hasText: "E2E Bitcoin Current Balance" });
  await assertBitcoinBalanceRows(mobileWalletCard);
  await expect(mobileWalletCard.locator(".wallet-value-summary")).toHaveCount(0);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END bitcoin-current-balance");
});

async function setMockCentralScenario(request, scenario) {
  const response = await request.post(`${MOCK_CENTRAL_URL}/__mock/scenario`, {
    data: scenario,
  });
  expect(response.ok(), `mock central scenario ${JSON.stringify(scenario)}`).toBeTruthy();
}

function freeProductOptionsResponse(accountLimit) {
  return {
    catalog_schema_version: 4,
    tiers: [
      {
        tier: "free",
        display_name: "Free",
        capability_schema_version: 3,
        capabilities: {
          limits: {
            accounts: { total: accountLimit },
            history: { max_transactions_per_account: 0 },
          },
          features: {
            balance_sync: true,
            exchange_rates_current: true,
            exchange_rates_history: false,
            transaction_history_sync: false,
            price_overrides: false,
            balance_assertions: false,
            hledger_export: false,
            tax_reports: false,
          },
        },
        presentation: {
          summary: `Local ownership and ${accountLimit} balance-only synced accounts.`,
          bullets: [`**${accountLimit}** balance-synced accounts`],
          is_featured: false,
          ribbon_label: null,
        },
        purchase_options: [],
      },
    ],
  };
}

test("wallets page renders actions and empty state for a new user", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  await page.goto("/wallets");

  await expect(page.getByTestId("wallets-title")).toHaveText("Wallets");
  await expect(page.getByTestId("wallets-subtitle")).toHaveText(
    "Your accounts, in one place.",
  );

  // Open the + Add dropdown to verify action items
  await page.getByTestId("wallets-add-button").click();
  // Trezor linking is temporarily disabled (TREZOR_LINK_ENABLED = false)
  await expect(page.getByTestId("wallets-action-link-trezor")).not.toBeVisible();
  await expect(page.getByTestId("wallets-action-add-xpub")).toBeVisible();
  await expect(page.getByTestId("wallets-action-add-bitcoin-address")).toBeVisible();
  await expect(page.getByTestId("wallets-action-add-ethereum-address")).toBeVisible();

  await expect(page.getByTestId("wallets-empty-state-title")).toHaveText(
    "No wallets linked yet.",
  );
  await expect(page.getByTestId("wallets-empty-state-body")).toHaveText(
    "Use the + Add button above to get started.",
  );

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("free wallet account limit follows product options free tier", async ({
  page,
  request,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  await setMockCentralScenario(request, {
    productOptionsResponse: freeProductOptionsResponse(22),
  });
  await page.goto("/payments");
  await expect(page.getByTestId("payments-tier-card-free")).toBeVisible();

  const addAddressResponse = await page.request.post("/_app/user/wallets/ethereum/add", {
    data: {
      request: {
        address: TEST_ETH_ADDRESS,
        network: "mainnet",
        wallet_label: "E2E Free Limit Wallet",
      },
    },
  });
  expect(addAddressResponse.ok()).toBeTruthy();

  await page.goto("/wallets");
  await expect(page.getByText(/1 of 22 active accounts used/i)).not.toBeVisible();

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("wallets add-bitcoin modal performs client-side validation before submit", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  await page.goto("/wallets");
  await page.getByTestId("wallets-add-button").click();
  await page.getByTestId("wallets-action-add-bitcoin-address").click();

  const modal = page
    .locator(".modal")
    .filter({ has: page.getByRole("heading", { name: "Add Bitcoin Address" }) });
  await expect(modal).toBeVisible();

  await modal.getByRole("button", { name: "Add Address" }).click();
  await expect(modal).toContainText("Bitcoin address is required.");

  await modal
    .getByPlaceholder("bc1q... / 1... / 3...")
    .fill("tb1q12345678901234567890123456789");
  await modal.getByRole("button", { name: "Add Address" }).click();
  await expect(modal).toContainText(
    "Bitcoin mainnet addresses usually start with bc1, 1, or 3.",
  );

  await modal.getByRole("button", { name: "Cancel" }).click();
  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("wallets add-ethereum modal hides api-key hint when key is configured and validates input", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  await page.goto("/wallets");
  await page.getByTestId("wallets-add-button").click();
  await page.getByTestId("wallets-action-add-ethereum-address").click();

  const modal = page
    .locator(".modal")
    .filter({ has: page.getByRole("heading", { name: "Add Ethereum Address" }) });
  await expect(modal).toBeVisible();
  // API key is configured via mock servers, so the hint should not appear
  await expect(modal).not.toContainText(
    "Ethereum transaction fetching requires an Etherscan API key.",
  );

  await modal.getByRole("button", { name: "Add Address" }).click();
  await expect(modal).toContainText("Ethereum address is required.");

  await modal.getByPlaceholder("0x...").fill("0x123");
  await modal.getByRole("button", { name: "Add Address" }).click();
  await expect(modal).toContainText(
    "Ethereum address must be 42 characters (0x + 40 hex).",
  );

  await modal.getByRole("button", { name: "Cancel" }).click();
  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
