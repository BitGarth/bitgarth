// Page manifest for screenshot capture.
// Each entry defines a page to capture, its route, auth requirement,
// optional data setup function, and a CSS selector to wait for before capture.
//
// Setup functions receive (requestContext, mockServers, setupContext).
// The setupContext object is shared across all authenticated pages in a run,
// allowing earlier setup functions to pass data (e.g., account IDs) to later ones.
// If a setup function returns { path: "..." }, that path overrides the static path.

import { TEST_ETH_ADDRESS, TEST_BTC_ADDRESS } from "../helpers/auth.mjs";

const SHOWCASE_WALLET_LABEL = "Garth's Main Wallet";
export const HOLDINGS_REPORT_OTHER_BTC_ADDRESS =
  "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";
const HOLDINGS_REPORT_OTHER_WALLET_LABEL = "Garth's Other Wallet";

async function postOk(requestContext, path, body) {
  const response = await requestContext.post(path, { data: body });
  if (!response.ok()) {
    throw new Error(`POST ${path} failed: ${response.status()}`);
  }
  // Void endpoints (e.g. settings) return an empty/`null` body.
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

// Activates the basic paid plan via the mock-central payment flow so synced
// accounts unlock transaction history. No-op under external --base-url runs
// (no mock-central handle). Idempotent.
async function activateBasicPlan(requestContext, ctx) {
  if (!ctx.mockCentral || ctx.planActivated) {
    return;
  }

  const scenario = await requestContext.post(
    `${ctx.mockCentral.baseUrl}/__mock/scenario`,
    {
      data: {
        orderStatus: "paid",
        paidTokenPayload: { premium_access_token: "test-token", tier: "basic" },
      },
    },
  );
  if (!scenario.ok()) {
    throw new Error(`mock-central scenario failed: ${scenario.status()}`);
  }

  const start = await postOk(requestContext, "/_app/user/payments/premium/start", {
    product_option_id: "basic_12_months_usd",
  });
  const orderId = start?.central_order_id;
  if (!orderId) {
    throw new Error("premium/start did not return central_order_id");
  }
  await postOk(requestContext, "/_app/user/payments/premium/poll", { order_id: orderId });

  ctx.planActivated = true;
}

// Builds the showcase wallet used for the populated screenshots: one realistic
// Bitcoin account, one Ethereum account (both synced against the mock servers),
// and one Monero manual asset. Currency is EUR with price fetching enabled so
// the net-worth indicator and per-asset EUR conversions render. Idempotent so
// the kebab-menu page can reuse the same wallet.
async function ensurePopulatedWallet(requestContext, ctx) {
  if (ctx.walletId) {
    return;
  }

  // Plan must be active before addresses are added so their auto-enqueued sync
  // includes transaction history.
  await activateBasicPlan(requestContext, ctx);

  // EUR + price fetching so balances convert and net worth shows.
  await postOk(requestContext, "/_app/user/settings/currency", {
    currency: "EUR",
  });
  await postOk(requestContext, "/_app/user/preferences/price_fetching", {
    enabled: true,
  });

  const eth = await postOk(requestContext, "/_app/user/wallets/ethereum/add", {
    request: {
      address: TEST_ETH_ADDRESS,
      network: "mainnet",
      wallet_label: SHOWCASE_WALLET_LABEL,
    },
  });
  ctx.walletId = eth.wallet_id;
  ctx.ethAccountId = eth.account_id;

  const btc = await postOk(requestContext, "/_app/user/wallets/bitcoin/add", {
    request: {
      address: TEST_BTC_ADDRESS,
      network: "mainnet",
      wallet_id: ctx.walletId,
    },
  });
  ctx.btcAccountId = btc.account_id;

  const monero = await postOk(
    requestContext,
    "/_app/user/wallets/manual-assets/add",
    {
      request: {
        wallet_id: ctx.walletId,
        asset_instance_id: {
          asset_id: "monero",
          network_id: "monero-mainnet",
        },
      },
    },
  );

  // Monero is manual. The assertions keep the balance flat through the 2024
  // report, then show a balance decrease by the end of 2025 without any
  // transaction history.
  for (const assertion of [
    { asserted_on: "2023-12-20", balance: "10.409462" },
    { asserted_on: "2024-12-31", balance: "10.409462" },
    { asserted_on: "2025-12-31", balance: "8.473921" },
  ]) {
    await postOk(requestContext, "/_app/user/manual-asset-assertions/add", {
      request: {
        account_id: monero.account_id,
        asserted_on: assertion.asserted_on,
        balance: assertion.balance,
        note: null,
      },
    });
  }

  // Adding addresses enqueues automatic sync; trigger an explicit user-scope
  // sync as well, then wait until all three assets are priced before capture.
  await postOk(requestContext, "/_app/user/transactions/sync", {
    request: { source: "manual", scope: { kind: "user" } },
  });

  // Block until synced transaction history lands so the transactions page and
  // the report render real data.
  await waitForConfirmedTransactions(requestContext, ctx.btcAccountId, 2, 30_000);
  await waitForConfirmedTransactions(requestContext, ctx.ethAccountId, 2, 30_000);

  // Net worth ≈ 0.30407540·92000 + 1.197007·3200 + 8.473921·280 ≈ €34,178.
  // Require all three priced and a total only reachable once BTC + ETH sync
  // lands, avoiding a half-synced (zero-balance) capture.
  await waitForWalletValuation(requestContext, {
    expectedPriced: 3,
    minTotal: 30_000,
    timeoutMs: 30_000,
  });
}

async function ensureHoldingsReportOtherWallet(requestContext, ctx) {
  if (ctx.holdingsReportOtherWalletId) {
    return;
  }

  const btc = await postOk(requestContext, "/_app/user/wallets/bitcoin/add", {
    request: {
      address: HOLDINGS_REPORT_OTHER_BTC_ADDRESS,
      network: "mainnet",
      wallet_label: HOLDINGS_REPORT_OTHER_WALLET_LABEL,
    },
  });
  ctx.holdingsReportOtherWalletId = btc.wallet_id;
  ctx.holdingsReportOtherBtcAccountId = btc.account_id;

  await postOk(requestContext, "/_app/user/transactions/sync", {
    request: { source: "manual", scope: { kind: "user" } },
  });

  await waitForConfirmedTransactions(
    requestContext,
    ctx.holdingsReportOtherBtcAccountId,
    1,
    30_000,
  );
}

async function waitForConfirmedTransactions(requestContext, accountId, expected, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let last = -1;
  while (Date.now() < deadline) {
    const response = await requestContext.get(
      `/_app/user/account/${accountId}/transactions?pending_page=1&confirmed_page=1`,
    );
    if (response.ok()) {
      const body = await response.json();
      last = body?.confirmed?.total ?? -1;
      if (last >= expected) {
        return;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for ${expected} confirmed tx on account ${accountId}; last total=${last}`,
  );
}

async function waitForWalletValuation(
  requestContext,
  { expectedPriced, minTotal, timeoutMs },
) {
  const deadline = Date.now() + timeoutMs;
  let lastSummary = null;
  while (Date.now() < deadline) {
    const response = await requestContext.get("/_app/user/wallets");
    if (response.ok()) {
      const body = await response.json();
      lastSummary = body.value_summary;
      if (
        lastSummary &&
        lastSummary.priced_asset_count >= expectedPriced &&
        Number.parseFloat(lastSummary.priced_total) >= minTotal
      ) {
        return;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(
    `Timed out waiting for wallet valuation (>=${expectedPriced} priced, total>=${minTotal}); last value_summary=${JSON.stringify(lastSummary)}`,
  );
}

const REPORTS = {
  "2024": {
    from: "2024-01-01",
    to: "2024-12-31",
    prices: {
      bitcoin: { Opening: "40018.25", Closing: "91212.60" },
      ethereum: { Opening: "2131.35", Closing: "3239.60" },
      monero: { Opening: "153.67", Closing: "188.77" },
    },
  },
  "2025": {
    from: "2025-01-01",
    to: "2025-12-31",
    prices: {
      bitcoin: { Opening: "91212.60", Closing: "75517.31" },
      ethereum: { Opening: "3239.60", Closing: "2528.90" },
      monero: { Opening: "188.77", Closing: "230.26" },
    },
  },
};

// Requested EUR boundary prices keyed by catalog asset id.
function boundaryPriceEur(report, subject, boundary) {
  const id = (subject?.id ?? "").toLowerCase();
  const entry = report.prices[id] ?? { Opening: "100", Closing: "110" };
  return entry[boundary];
}

// Place each override at the start of the boundary's local day. The resolver
// picks the latest override at-or-before the boundary instant within the same
// day, and the opening boundary instant is that day's 00:00:00 — so a midday
// override would fall after it and be ignored. 00:00:00 is <= any instant on
// the day, so it works for both opening and closing boundaries.
function nextUtcDate(date) {
  const value = new Date(`${date}T00:00:00Z`);
  value.setUTCDate(value.getUTCDate() + 1);
  return value.toISOString().slice(0, 10);
}

function boundaryTimeLocal(report, boundary, useNextDayForClosing = false) {
  if (boundary === "Opening") {
    return `${report.from}T00:00:00`;
  }
  const date = useNextDayForClosing ? nextUtcDate(report.to) : report.to;
  return `${date}T00:00:00`;
}

async function fetchMissingResolvedPrices(requestContext, walletId, report) {
  const response = await requestContext.get(
    `/_app/user/wallets/${walletId}/report/resolved-prices?from=${report.from}&to=${report.to}&timezone=UTC`,
  );
  if (!response.ok()) {
    throw new Error(`resolved-prices fetch failed: ${response.status()}`);
  }
  const views = await response.json();
  return views.filter((view) => view.price === null);
}

// Seed user price overrides for every report boundary that lacks a price so the
// wallet report's price section collapses and the main report renders. Throws
// if a boundary remains unpriced after seeding (with a closing-day fallback).
async function seedReportPrices(requestContext, ctx, reportKey) {
  const seededKey = `reportPricesSeeded${reportKey}`;
  if (ctx[seededKey]) {
    return;
  }
  const report = REPORTS[reportKey];

  const seed = async (view, useNextDayForClosing) => {
    // The server fn arg is named `input`, so the body is wrapped accordingly.
    await postOk(requestContext, "/_app/user/prices/overrides", {
      input: {
        subject: view.subject,
        quote_currency: "EUR",
        price_time_local: boundaryTimeLocal(report, view.boundary, useNextDayForClosing),
        price: boundaryPriceEur(report, view.subject, view.boundary),
        source_note: null,
      },
    });
  };

  for (const view of await fetchMissingResolvedPrices(requestContext, ctx.walletId, report)) {
    await seed(view, false);
  }

  // Closing boundary day can be exclusive; retry any remainder on the next day.
  let remaining = await fetchMissingResolvedPrices(requestContext, ctx.walletId, report);
  for (const view of remaining) {
    await seed(view, true);
  }

  remaining = await fetchMissingResolvedPrices(requestContext, ctx.walletId, report);
  if (remaining.length > 0) {
    throw new Error(
      `report prices still missing after seeding: ${JSON.stringify(remaining)}`,
    );
  }

  ctx[seededKey] = true;
}

export const pages = [
  {
    name: "login",
    path: "/login",
    auth: false,
    setup: null,
    waitFor: "#username",
  },
  {
    name: "register",
    path: "/register",
    auth: false,
    setup: null,
    waitFor: "#password",
  },
  {
    name: "wallets-empty",
    path: "/wallets",
    auth: true,
    setup: null,
    waitFor: "[data-testid='wallets-title']",
  },
  {
    name: "wallets-add-open",
    path: "/wallets",
    auth: true,
    setup: null,
    waitFor: "[data-testid='wallets-title']",
    interact: async (page) => {
      await page.getByTestId("wallets-add-button").click();
      await page.getByTestId("wallets-action-add-bitcoin-address").waitFor({ state: "visible", timeout: 5000 });
    },
  },
  {
    name: "settings",
    path: "/settings?section=account",
    auth: true,
    setup: null,
    waitFor: ".account-identity",
  },
  {
    name: "exports-hledger",
    path: "/exports/hledger",
    auth: true,
    setup: null,
    waitFor: ".page-title",
  },
  {
    name: "payments",
    path: "/payments",
    auth: true,
    // Must be captured before any page activates the plan, so it shows the
    // purchase options rather than an active subscription. Fail loud if a
    // reordering ever breaks that invariant.
    setup: async (_requestContext, _mockServers, ctx) => {
      if (ctx.planActivated) {
        throw new Error("payments must be captured before plan activation");
      }
    },
    waitFor: "[data-testid='payments-card']",
  },
  {
    name: "wallets-populated",
    path: "/wallets",
    auth: true,
    setup: async (requestContext, _mockServers, ctx) => ensurePopulatedWallet(requestContext, ctx),
    waitFor: ".wallet-value-overview",
  },
  {
    name: "wallets-kebab-open",
    path: "/wallets",
    auth: true,
    setup: async (requestContext, _mockServers, ctx) => ensurePopulatedWallet(requestContext, ctx),
    waitFor: ".wallet-card",
    interact: async (page) => {
      await page.locator(".kebab-menu-trigger").first().click();
      await page.locator(".kebab-menu-dropdown.visible").waitFor({ state: "visible", timeout: 5000 });
    },
  },
  {
    name: "wallet-report-2024",
    path: "/wallets",
    auth: true,
    setup: async (requestContext, _mockServers, ctx) => {
      await ensurePopulatedWallet(requestContext, ctx);
      await seedReportPrices(requestContext, ctx, "2024");
      return {
        path: `/wallets/${ctx.walletId}?start=${REPORTS["2024"].from}&end=${REPORTS["2024"].to}`,
      };
    },
    waitFor: ".wr-prices-section",
  },
  {
    name: "wallet-report-2025",
    path: "/wallets",
    auth: true,
    setup: async (requestContext, _mockServers, ctx) => {
      await ensurePopulatedWallet(requestContext, ctx);
      await seedReportPrices(requestContext, ctx, "2025");
      return {
        path: `/wallets/${ctx.walletId}?start=${REPORTS["2025"].from}&end=${REPORTS["2025"].to}`,
      };
    },
    waitFor: ".wr-prices-section",
  },
  {
    name: "holdings-report-2024",
    path: "/reports/holdings",
    auth: true,
    setup: async (requestContext, _mockServers, ctx) => {
      await ensurePopulatedWallet(requestContext, ctx);
      await ensureHoldingsReportOtherWallet(requestContext, ctx);
      await seedReportPrices(requestContext, ctx, "2024");
      return {
        path: `/reports/holdings?start=${REPORTS["2024"].from}&end=${REPORTS["2024"].to}`,
      };
    },
    waitFor: ".wr-desktop-view:visible, .wr-mobile-view:visible",
  },
  {
    name: "account-transactions",
    path: "/wallets",
    auth: true,
    setup: async (requestContext, _mockServers, ctx) => {
      await ensurePopulatedWallet(requestContext, ctx);
      return { path: `/wallets/account/${ctx.btcAccountId}/transactions` };
    },
    waitFor: ".transactions-list",
  },
];
