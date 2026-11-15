import { expect, test } from "../helpers/mock-fixture.mjs";
import { formatDefaultUtcTimestamp } from "../helpers/timestamps.mjs";
import {
  TEST_ETH_ADDRESS,
  addAndSyncLimitedBitcoinAccount,
  assertNoBrowserDiagnostics,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

async function expectLastConfirmation(note, observedAt) {
  await expect(note).toBeVisible();
  await expect(note).toContainText("Last confirmed 0.00095 BTC");
  const time = note.locator("time");
  await expect(time).toBeVisible();
  await expect(time).toHaveText(formatDefaultUtcTimestamp(observedAt));
  expect(Date.parse(await time.getAttribute("datetime"))).toBe(Date.parse(observedAt));
}

test("limited Bitcoin reports show the last confirmation beside an unknown closing value on desktop and mobile", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START limited-bitcoin-report");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  const walletLabel = "E2E Limited Bitcoin Report";
  const { wallet_id: walletId, account_id: accountId } = await addAndSyncLimitedBitcoinAccount(
    page,
    mockServers,
    walletLabel,
  );
  const response = await page.request.get(`/_app/user/wallets/${walletId}/report?timezone=UTC`);
  expect(response.ok()).toBeTruthy();
  const { accounts } = await response.json();
  const accountReport = accounts.find((account) => account.account_id === accountId);
  const observedAt = accountReport?.last_confirmed_bitcoin?.observed_at;
  expect(Date.parse(observedAt)).not.toBeNaN();

  for (const mobile of [false, true]) {
    if (mobile) {
      await page.setViewportSize({ width: 390, height: 844 });
    }
    const row = page.locator(mobile ? ".wr-card" : ".wr-row");
    const valueCell = mobile ? ".wr-card-side" : ".wr-td-value";
    const fiat = mobile ? ".wr-card-fiat-value" : ".wr-fiat-value";
    await page.goto(`/wallets/${walletId}`);
    const account = row.filter({ hasText: "Limited Bitcoin" });
    await expect(account).toBeVisible();
    await expect(account).toContainText("History coverage: Coverage limited");
    await expect(account.locator(mobile ? ".wr-card-crypto" : ".wr-crypto-value")).toHaveText([
      "Not available", "Not available",
    ]);
    await expect(account.locator(fiat)).toHaveCount(0);
    const accountNote = account.locator(valueCell).nth(1).locator(".wr-last-confirmed");
    await expectLastConfirmation(accountNote, observedAt);

    await page.goto("/reports/holdings");
    const wallet = row.filter({ hasText: walletLabel });
    await expect(wallet).toBeVisible();
    await expect(wallet.locator(fiat)).toHaveText(["—", "—"]);
    const walletNote = wallet.locator(valueCell).nth(1).locator(".wr-last-confirmed");
    await expect(walletNote).toContainText("Limited Bitcoin");
    await expectLastConfirmation(walletNote, observedAt);
    await expect(page.getByRole("button", { name: /§ Prices/ })).toHaveCount(0);
  }

  const summary = page.locator(".wr-mobile-view .wr-summary-card");
  await expect(summary).toBeVisible();
  await expect(summary.locator(".wr-summary-value")).toHaveText(["—", "—"]);
  await expect(summary.locator(".wr-gap-note")).toHaveText([
    "Not every wallet has a value for this date",
    "Not every wallet has a value for this date",
  ]);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END limited-bitcoin-report");
});

test("wallet report canonicalizes missing route and keeps the selected range", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");

  await registerViaUiAndExpectAuthenticated(page);

  const addAddressResponse = await page.request.post("/_app/user/wallets/ethereum/add", {
    data: {
      request: {
        address: TEST_ETH_ADDRESS,
        network: "mainnet",
        wallet_label: "E2E Wallet Report",
      },
    },
  });
  expect(addAddressResponse.ok()).toBeTruthy();
  const addAddressPayload = await addAddressResponse.json();
  const walletId = addAddressPayload.wallet_id;
  expect(walletId).toBeTruthy();

  const reportPage = await page.context().newPage();
  const diagnostics = await attachBrowserDiagnostics(reportPage, testInfo);

  await reportPage.goto(`/wallets/${walletId}`);
  await expect(
    reportPage.getByRole("heading", { name: /^E2E Wallet Report\b/ }),
  ).toBeVisible();

  await expect
    .poll(() => {
      const url = new URL(reportPage.url());
      return Boolean(url.searchParams.get("start") && url.searchParams.get("end"));
    })
    .toBe(true);

  const canonicalUrl = new URL(reportPage.url());
  const canonicalStart = canonicalUrl.searchParams.get("start");
  const canonicalEnd = canonicalUrl.searchParams.get("end");
  const report = await reportPage.request.get(`/_app/user/wallets/${walletId}/report?timezone=UTC`);
  expect(report.ok()).toBeTruthy();
  const { access } = await report.json();
  expect(canonicalStart).toBe(access.requested_from);
  expect(canonicalEnd).toBe(access.requested_to);
  expect(canonicalUrl.searchParams.get("from")).toBeNull();
  expect(canonicalUrl.searchParams.get("to")).toBeNull();

  // Clamped ranges already open the custom date fields.
  const customToggle = reportPage.locator(".date-range-custom-toggle");
  if ((await customToggle.getAttribute("aria-expanded")) !== "true") {
    await customToggle.click();
  }

  const startInput = reportPage.locator("#wallet-report-start");
  const endInput = reportPage.locator("#wallet-report-end");
  await expect(startInput).toHaveValue(access.effective_from);
  await expect(endInput).toHaveValue(access.effective_to);

  const previousYear = Number(canonicalStart.slice(0, 4)) - 1;
  const previousYearStart = `${previousYear}-01-01`;
  const previousYearEnd = `${previousYear}-12-31`;

  // Set the end first so the intermediate, inverted range does not navigate.
  await endInput.fill(previousYearEnd);
  await startInput.fill(previousYearStart);
  await startInput.press("Tab");

  await expect
    .poll(() => {
      const url = new URL(reportPage.url());
      return `${url.searchParams.get("start")}|${url.searchParams.get("end")}`;
    })
    .toBe(`${previousYearStart}|${previousYearEnd}`);

  await expect(reportPage.locator(".wr-free-window-badge")).toContainText(
    "Free window applied",
  );
  // The dial names the year the user chose, even though the Free window clamps it.
  await expect(reportPage.locator(".date-range-year-value")).toHaveText(String(previousYear));

  assertNoBrowserDiagnostics(diagnostics);
  await reportPage.close();
  await markTestBoundary(testInfo, "END");
});

test("holdings report canonicalizes a missing range", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  const report = await page.request.get("/_app/user/reports/holdings?timezone=UTC");
  expect(report.ok()).toBeTruthy();
  const { access } = await report.json();
  await page.goto("/reports/holdings");
  await expect(page.getByRole("heading", { name: /^Holdings Report\b/, level: 1 })).toBeVisible();

  await expect
    .poll(() => {
      const url = new URL(page.url());
      return [url.searchParams.get("start"), url.searchParams.get("end")];
    })
    .toEqual([access.requested_from, access.requested_to]);
});
