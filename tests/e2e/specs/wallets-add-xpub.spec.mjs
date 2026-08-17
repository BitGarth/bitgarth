import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  configureMockServers,
  registerViaUiAndExpectAuthenticated,
  TEST_XPUB,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

test("add xpub via modal creates wallet and shows it in wallet list", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  await page.goto("/wallets");

  // Open the + Add dropdown, then the Add Extended Public Key modal
  await page.getByTestId("wallets-add-button").click();
  await page.getByTestId("wallets-action-add-xpub").click();
  const modal = page
    .locator(".modal")
    .filter({ has: page.getByRole("heading", { name: "Add Bitcoin Extended Public Key" }) });
  await expect(modal).toBeVisible();

  // Step 1: Paste xpub and click Next
  await modal.locator(".xpub-input").fill(TEST_XPUB);
  await modal.getByRole("button", { name: "Next" }).click();

  // Step 2: Wait for validation results
  await expect(modal.locator(".xpub-scheme-picker")).toBeVisible({ timeout: 15_000 });

  // Verify the three address schemes appear
  const schemeOptions = modal.locator(".xpub-scheme-option");
  await expect(schemeOptions).toHaveCount(3);
  await expect(modal.locator(".xpub-scheme-name").nth(0)).toHaveText("Legacy");
  await expect(modal.locator(".xpub-scheme-name").nth(1)).toHaveText("SegWit Compatible");
  await expect(modal.locator(".xpub-scheme-name").nth(2)).toHaveText("Native SegWit");

  // Enter a wallet label and submit
  await modal.locator("#wallet_label").fill("E2E Test Wallet");
  await modal.getByRole("button", { name: "Add" }).click();

  // Submit closes the modal directly — there is no confirmation step
  await expect(modal).not.toBeVisible({ timeout: 15_000 });

  // Verify the new wallet appears in the list
  const walletCard = page.locator(".wallet-card").filter({ hasText: "E2E Test Wallet" });
  await expect(walletCard).toBeVisible({ timeout: 10_000 });

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("same-xpub accounts show distinct scheme sublines", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START xpub-sublines");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);

  const addLegacy = await page.request.post("/_app/user/wallets/xpub/add", {
    data: {
      request: {
        extended_pubkey: TEST_XPUB,
        address_scheme: "legacy",
        wallet_label: "E2E Subline Wallet",
      },
    },
  });
  expect(addLegacy.ok()).toBeTruthy();
  const addLegacyPayload = await addLegacy.json();
  const walletId = addLegacyPayload.wallet_id;
  expect(walletId).toBeTruthy();

  const addNativeSegwit = await page.request.post("/_app/user/wallets/xpub/add", {
    data: {
      request: {
        extended_pubkey: TEST_XPUB,
        address_scheme: "native_segwit",
        wallet_id: walletId,
      },
    },
  });
  expect(addNativeSegwit.ok()).toBeTruthy();

  await page.goto("/wallets");
  const sublines = page.getByTestId("account-row-subline");
  await expect(sublines).toHaveCount(2);
  await expect(sublines.filter({ hasText: "Legacy" })).toHaveCount(1);
  await expect(sublines.filter({ hasText: "Native SegWit" })).toHaveCount(1);
  await expect(sublines.first()).toContainText("…");

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END xpub-sublines");
});
