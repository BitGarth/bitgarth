import { expect, test } from "../helpers/mock-fixture.mjs";
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

test("limited Bitcoin report stays unavailable without fiat on desktop and mobile", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START limited-bitcoin-report");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  const { wallet_id: walletId } = await addAndSyncLimitedBitcoinAccount(
    page,
    mockServers,
    "E2E Limited Bitcoin Report",
  );

  await page.goto(`/wallets/${walletId}`);
  const desktopRow = page.locator(".wr-row").filter({ hasText: "Limited Bitcoin" });
  await expect(desktopRow).toContainText("History coverage: Coverage limited");
  await expect(desktopRow.locator(".wr-crypto-value")).toHaveText([
    "Not available",
    "Not available",
  ]);
  await expect(desktopRow.locator(".wr-fiat-value")).toHaveCount(0);

  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  const mobileCard = page.locator(".wr-card").filter({ hasText: "Limited Bitcoin" });
  await expect(mobileCard).toContainText("History coverage: Coverage limited");
  await expect(mobileCard.locator(".wr-card-crypto")).toHaveText([
    "Not available",
    "Not available",
  ]);
  await expect(mobileCard.locator(".wr-card-fiat-value")).toHaveCount(0);

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
  expect(canonicalStart).toBeTruthy();
  expect(canonicalEnd).toBeTruthy();
  expect(canonicalUrl.searchParams.get("from")).toBeNull();
  expect(canonicalUrl.searchParams.get("to")).toBeNull();

  // Date inputs live behind the "Custom range" disclosure now.
  await reportPage.locator(".date-range-custom-toggle").click();

  const startInput = reportPage.locator("#wallet-report-start");
  const endInput = reportPage.locator("#wallet-report-end");
  await expect(startInput).toHaveValue(canonicalStart);
  await expect(endInput).toHaveValue(canonicalEnd);

  const previousYear = Number(canonicalStart.slice(0, 4)) - 1;
  const previousYearStart = `${previousYear}-01-01`;
  const previousYearEnd = `${previousYear}-12-31`;

  await reportPage.getByRole("button", { name: "Previous year", exact: true }).click();

  await expect
    .poll(() => {
      const url = new URL(reportPage.url());
      return `${url.searchParams.get("start")}|${url.searchParams.get("end")}`;
    })
    .toBe(`${previousYearStart}|${previousYearEnd}`);

  await expect(reportPage.locator(".wr-free-window-badge")).toContainText(
    "Free window applied",
  );
  await expect(reportPage.locator(".date-range-year-value")).toHaveText("Custom range");

  assertNoBrowserDiagnostics(diagnostics);
  await reportPage.close();
  await markTestBoundary(testInfo, "END");
});
