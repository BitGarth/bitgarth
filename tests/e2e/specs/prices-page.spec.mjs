import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  activatePaidHistoryWithCap,
  assertNoBrowserDiagnostics,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

async function addManualMonero(request) {
  const response = await request.post("/_app/user/wallets/manual-assets/add", {
    data: {
      request: {
        wallet_label: "E2E Prices Page",
        asset_instance_id: { asset_id: "monero", network_id: "monero-mainnet" },
      },
    },
  });
  expect(response.ok()).toBeTruthy();
  return response.json();
}

async function setOwnMoneroPrice(request) {
  const history = await request.get("/_app/user/prices/history?asset=monero");
  expect(history.ok()).toBeTruthy();
  const { currency, resolved_from: from } = await history.json();
  const response = await request.post("/_app/user/prices/overrides", {
    data: {
      input: {
        subject: { kind: "catalog_asset", id: "monero" },
        quote_currency: currency,
        price_time_local: `${from}T00:00:00`,
        price: "123.45",
        source_note: null,
      },
    },
  });
  expect(response.ok()).toBeTruthy();
  return currency;
}

test("Free price history keeps the nine-month notice after resolving a missing range", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await addManualMonero(page.request);
  const enabled = await page.request.post("/_app/user/preferences/price_fetching", {
    data: { enabled: true },
  });
  expect(enabled.ok()).toBeTruthy();
  await page.goto("/wallets");

  // Give the initial request a default outside the Free window regardless of the
  // day the test runs. Let the server apply the real access rules on both reads.
  const start = "2001-01-01";
  const end = "2001-11-10";
  const historyPath = "/_app/user/prices/history";
  await page.route(
    (url) => url.pathname === historyPath && !url.searchParams.get("from"),
    async (route) => {
      const url = new URL(route.request().url());
      url.searchParams.set("from", start);
      url.searchParams.set("to", end);
      const response = await route.fetch({ url: url.href });
      const view = await response.json();
      await route.fulfill({
        response,
        json: { ...view, default_this_year_from: start, default_this_year_to: end },
      });
    },
  );

  const refetch = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return url.pathname === historyPath && url.searchParams.get("from") === start;
  });
  await page.locator(".sidebar").getByRole("link", { name: "Prices" }).click();
  const view = await (await refetch).json();
  expect(view.access.range_clamped).toBe(true);
  await expect(page).toHaveURL(
    (url) => url.searchParams.get("start") === start && url.searchParams.get("end") === end,
  );
  await expect(page.locator(".prices-scope-note")).toContainText("Free plans show the last 9 months");
  await expect(page.locator(".prices-empty")).toContainText("outside the last 9 months");
  await page.getByTestId("nav-price-control").locator(".nav-price-switch").click();
  await expect(page.locator(".prices-empty")).toContainText("You haven't set prices");
  await expect(page.locator(".prices-scope-note")).toHaveCount(0);
});

test("prices page shows own prices always and CoinGecko prices only once syncing is on", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  const { account_id: accountId } = await addManualMonero(page.request);
  await activatePaidHistoryWithCap(page.request, 100);
  await setOwnMoneroPrice(page.request);

  await page.goto("/wallets");
  await page.locator(".sidebar").getByRole("link", { name: "Prices" }).click();
  await expect(page).toHaveURL(/\/prices\?/);
  await expect(page.getByRole("heading", { name: "Prices", level: 1 })).toBeVisible();
  await expect(page.getByRole("group", { name: "Asset" }).locator(".tx-filter-pill.active")).toHaveText(
    "Monero",
  );
  await expect(page.locator(".prices-source-notice")).toContainText(
    "CoinGecko prices are hidden while CoinGecko syncing is off.",
  );
  await expect(page.locator(".prices-table .prices-source-own")).toHaveText("Your price");
  await expect(page.locator(".prices-table")).not.toContainText("CoinGecko");
  await expect(page.getByRole("group", { name: "Source" })).toHaveCount(0);

  const historyRequests = [];
  page.on("request", (request) => {
    if (request.url().includes("/_app/user/prices/history")) {
      historyRequests.push(request.url());
    }
  });

  const enabled = page.waitForResponse(
    (response) =>
      response.url().includes("/_app/user/preferences/price_fetching") &&
      response.request().method() === "POST",
  );
  await page.locator(".prices-source-switch").click();
  expect((await enabled).ok()).toBeTruthy();
  await expect(page.locator(".prices-source-notice")).toContainText("CoinGecko syncing is on.");
  await expect(page.getByTestId("nav-price-fetching-toggle")).toBeChecked();
  expect(historyRequests).toHaveLength(0);

  const disabledElsewhere = await page.request.post("/_app/user/preferences/price_fetching", {
    data: { enabled: false },
  });
  expect(disabledElsewhere.ok()).toBeTruthy();
  await page.getByRole("button", { name: "Show CoinGecko prices" }).click();
  await expect(page.locator(".prices-source-switch")).toBeVisible();
  await expect(page.getByTestId("nav-price-fetching-toggle")).not.toBeChecked();
  await expect(page.locator(".prices-table .prices-source-own")).toHaveText("Your price");

  await page.locator(".prices-source-switch").click();
  const loaded = page.waitForRequest((request) =>
    request.url().includes("/_app/user/prices/history"),
  );
  await page.getByRole("button", { name: "Show CoinGecko prices" }).click();
  await loaded;
  await expect(page.locator(".prices-source-notice")).toHaveCount(0);
  const sources = page.getByRole("group", { name: "Source" });
  await expect(sources.locator(".tx-filter-pill.active")).toHaveText("All");
  await sources.getByRole("button", { name: "Your prices" }).click();
  await expect(sources.locator(".tx-filter-pill.active")).toHaveText("Your prices");
  await expect(page.locator(".prices-table .prices-source-own")).toHaveText("Your price");

  await page.goto(`/wallets/account/${accountId}/transactions`);
  await page.getByRole("button", { name: /§ Account/ }).click();
  await page.getByRole("link", { name: /Monero price history/ }).click();
  await expect(page).toHaveURL(/\/prices\?asset=monero/);
  await expect(page.getByRole("group", { name: "Asset" }).locator(".tx-filter-pill.active")).toHaveText(
    "Monero",
  );

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("opting out from a populated CoinGecko filter brings back personal prices", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await addManualMonero(page.request);
  await activatePaidHistoryWithCap(page.request, 100);
  const currency = await setOwnMoneroPrice(page.request);
  const toggle = page.getByTestId("nav-price-control").locator(".nav-price-switch");
  await toggle.click();
  await expect(page.getByTestId("nav-price-fetching-toggle")).toBeChecked();

  // Supply a deterministic provider quote. The server still applies the real
  // saved preference, source filter and personal-price query on every request.
  await page.route("**/_app/user/prices/history?**", async (route) => {
    const response = await route.fetch();
    const view = await response.json();
    if (view.prices_enabled && view.source === "CoinGecko") {
      view.entries = [{ Daily: { date_utc: view.resolved_to, price: "98.76" } }];
      view.total_entries = 1;
    }
    await route.fulfill({ response, json: view });
  });
  await page.locator(".sidebar").getByRole("link", { name: "Prices" }).click();
  const sources = page.getByRole("group", { name: "Source" });
  await sources.getByRole("button", { name: "CoinGecko" }).click();
  const providerQuote = page.getByRole("button", { name: `Copy price 98.76 ${currency}` });
  await expect(providerQuote).toBeVisible();
  await toggle.click();
  await expect(page.getByTestId("nav-price-fetching-toggle")).not.toBeChecked();
  await expect(providerQuote).toHaveCount(0);
  await expect(sources).toHaveCount(0);
  await expect(page.locator(".prices-source-notice")).toContainText("CoinGecko prices are hidden");
  await expect(page.getByRole("button", { name: `Copy price 123.45 ${currency}` })).toBeVisible();
});

test("changing the reporting currency replaces the open price table and its copy controls", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await addManualMonero(page.request);
  await activatePaidHistoryWithCap(page.request, 100);
  const currency = await setOwnMoneroPrice(page.request);
  const emptyCurrency = currency === "USD" ? "EUR" : "USD";
  await page.getByTestId("nav-price-control").locator(".nav-price-switch").click();
  const selector = page.getByTestId("nav-price-currency-select");
  await expect(selector).toHaveValue(currency);
  await page.locator(".sidebar").getByRole("link", { name: "Prices" }).click();
  const ownFilter = page.getByRole("group", { name: "Source" }).getByRole("button", { name: "Your prices" });
  await ownFilter.click();
  await expect(ownFilter).toHaveAttribute("aria-pressed", "true");
  const quote = page.getByRole("button", { name: `Copy price 123.45 ${currency}` });
  await expect(quote).toBeVisible();

  // The old quote must already be unavailable while its replacement is loading.
  const pendingHistory = Promise.withResolvers();
  await page.route("**/_app/user/prices/history?**", async (route) => {
    await pendingHistory.promise;
    await route.continue();
  }, { times: 1 });
  try {
    await selector.selectOption(emptyCurrency);
    await expect(quote).toHaveCount(0);
  } finally {
    pendingHistory.resolve();
  }
  await expect(page.locator(".page-subtitle")).toContainText(`in ${emptyCurrency}, your reporting currency`);
  await expect(page.locator(".prices-empty")).toContainText("You haven't set prices");
  await expect(quote).toHaveCount(0);
  await selector.selectOption(currency);
  await expect(page.getByRole("heading", { name: `Monero in ${currency}` })).toBeVisible();
  await expect(quote).toBeVisible();
});

test("copying a price confirms only successful clipboard writes", async ({ page }, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await registerViaUiAndExpectAuthenticated(page);
  await addManualMonero(page.request);
  await activatePaidHistoryWithCap(page.request, 100);
  await setOwnMoneroPrice(page.request);

  await page.locator(".sidebar").getByRole("link", { name: "Prices" }).click();
  await expect(page).toHaveURL(/[?&]start=\d{4}-\d{2}-\d{2}/);
  const price = page.getByRole("button", { name: /^Copy price 123\.45 / });
  await expect(price).toBeVisible();
  await page.evaluate(() => {
    navigator.clipboard.writeText = () => new Promise((_, reject) => {
      window.denyPriceCopy = () => reject(new DOMException("Permission denied", "NotAllowedError"));
    });
  });
  await price.click();
  await page.waitForFunction(() => typeof window.denyPriceCopy === "function");
  await expect(price).not.toHaveClass(/\bcopied\b/);
  await expect(price).not.toContainText("Copied");

  await page.evaluate(() => window.denyPriceCopy());
  await expect(price).toHaveAttribute("data-full", "Copy failed");
  await expect(price).not.toHaveClass(/\bcopied\b/);
  await expect(price).not.toContainText("Copied");

  await page.evaluate(() => { delete navigator.clipboard.writeText; });
  await price.click();
  await expect(price).toHaveClass(/\bcopied\b/);
  await expect(price).toContainText("Copied 123.45");
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("123.45");

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
