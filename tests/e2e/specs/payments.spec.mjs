import { expect, test } from "../helpers/mock-fixture.mjs";
import { registerViaUiAndExpectAuthenticated } from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

const MOCK_CENTRAL_URL = "http://127.0.0.1:8082";

function productOption(id, quantity, unit, label, minorUnits, extra = {}) {
  return {
    id,
    term: {
      quantity,
      unit,
      label,
    },
    price: {
      minor_units: minorUnits,
      currency: "USD",
      currency_symbol: "$",
      display_scale: 2,
    },
    ...extra,
  };
}

function productTier(tier, displayName, slots, backfill, purchaseOptions, presentationOverrides = {}) {
  return {
    tier,
    display_name: displayName,
    capabilities: {
      limits: {
        synced_accounts: slots,
        history: {
          max_transactions_per_account: backfill,
        },
      },
    },
    presentation: {
      summary: `${displayName} test tier — ${slots} synced accounts.`,
      bullets: [`**${slots}** synced accounts`],
      ...presentationOverrides,
    },
    purchase_options: purchaseOptions,
  };
}

function productOptionsResponse(tiers, extra = {}) {
  return {
    catalog_schema_version: 4,
    tiers,
    ...extra,
  };
}

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

async function getMockCentralStatus(request) {
  const response = await request.get(`${MOCK_CENTRAL_URL}/__mock/status`);
  expect(response.ok(), "mock central status").toBeTruthy();
  return response.json();
}

async function getSingleMockCentralOrder(request) {
  let order = null;
  await expect(async () => {
    const status = await getMockCentralStatus(request);
    expect(status.orders).toHaveLength(1);
    order = status.orders[0];
  }).toPass({ timeout: 10_000 });
  return order;
}

async function installAtlosStub(page, { autoComplete = false } = {}) {
  await page.addInitScript(
    ({ autoComplete }) => {
      const stub = {
        __lastCall: null,
        Pay(options) {
          stub.__lastCall = options;
          window.__atlosCalls = window.__atlosCalls || [];
          window.__atlosCalls.push({
            merchantId: options.merchantId,
            orderId: options.orderId,
            orderAmount: options.orderAmount,
            orderCurrency: options.orderCurrency,
          });
          window.__atlosCompleted = () => options.onCompleted?.();
          window.__atlosCanceled = () => options.onCanceled?.();
          if (autoComplete) {
            setTimeout(() => options.onCompleted?.(), 50);
          }
        },
      };
      Object.defineProperty(window, "atlos", {
        value: stub,
        writable: false,
        configurable: true,
      });
    },
    { autoComplete },
  );
  await page.route("https://atlos.io/packages/app/atlos.js", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/javascript",
      body: "// stubbed by test",
    });
  });
}

async function latestAtlosOrderId(page) {
  let orderId = null;
  await expect(async () => {
    orderId = await page.evaluate(() => {
      const calls = window.__atlosCalls ?? [];
      return calls[calls.length - 1]?.orderId ?? null;
    });
    expect(orderId).toBeTruthy();
  }).toPass({ timeout: 10_000 });
  return orderId;
}

test.beforeEach(async ({ page, request }) => {
  await resetMockCentral(request);
  await installAtlosStub(page);
});

test("payments page shows paid plan terms before checkout", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  await expect(page.getByTestId("payments-paid-plan-terms")).toContainText(
    "By continuing, you agree to the paid plan terms.",
  );
});

test("upgrade click launches ATLOS with only server-issued values", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  const card = page.getByTestId("payments-card");
  await expect(card).toBeVisible();
  await expect(card).toHaveAttribute("data-status", "not_active");
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("$500");
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("/ 1 year");

  await page.getByTestId("payments-buy-btn-premium").click();

  await expect(async () => {
    const calls = await page.evaluate(() => window.__atlosCalls ?? []);
    const status = await getMockCentralStatus(request);
    expect(calls.length).toBeGreaterThan(0);
    expect(status.orders).toHaveLength(1);
    expect(calls[0].merchantId).toBeTruthy();
    expect(calls[0].orderId).toBe(status.orders[0].payment_attempt_id);
    expect(calls[0].orderId).not.toBe(status.orders[0].order_id);
    expect(calls[0].orderAmount).toBe(500);
    expect(calls[0].orderCurrency).toBe("USD");
  }).toPass({ timeout: 10_000 });

  await expect(card).toHaveAttribute("data-status", "pending", { timeout: 5_000 });

  await markTestBoundary(testInfo, "END");
});

test("payments page renders multiple options and starts checkout with the selected one", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await setMockCentralScenario(request, {
    productOptionsResponse: productOptionsResponse([
      productTier(
        "basic",
        "Basic",
        10,
        1000,
        [productOption("basic_12_months_usd", 12, "month", "1 year", 50, { presentation: { is_default: true } })],
        { is_featured: true, ribbon_label: "Early adopter discount" },
      ),
      productTier("premium", "Premium", 50, 50000, [
        productOption("premium_12_months_usd", 12, "month", "1 year", 123, { presentation: { is_default: true } }),
        productOption("premium_test_1_day_usd", 1, "day", "1 day (test)", 1),
      ]),
    ]),
  });

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  await expect(page.getByTestId("payments-tier-grid")).toBeVisible();
  await expect(page.getByTestId("payments-tier-card-basic")).toBeVisible();
  await expect(page.getByTestId("payments-tier-card-premium")).toBeVisible();

  // Premium card defaults to $1.23 / 1 year (server is_default). The
  // alternate term option surfaces as a clickable term-toggle inside the
  // Premium card.
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("$1.23");
  await expect(page.getByTestId("payments-term-toggle-premium_test_1_day_usd")).toBeVisible();

  // Flip Premium's term toggle to the 1-day option.
  await page.getByTestId("payments-term-toggle-premium_test_1_day_usd").click();
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("$0.01");
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("/ 1 day (test)");
  await expect(page.getByTestId("payments-buy-btn-premium")).toContainText("$0.01");
  await expect(page.getByTestId("payments-buy-btn-premium")).toContainText("1 day (test)");

  await page.getByTestId("payments-buy-btn-premium").click();

  await expect(async () => {
    const calls = await page.evaluate(() => window.__atlosCalls ?? []);
    expect(calls.length).toBeGreaterThan(0);
    expect(calls[0].orderAmount).toBe(0.01);
  }).toPass({ timeout: 10_000 });

  await expect(async () => {
    const status = await getMockCentralStatus(request);
    expect(status.orders).toHaveLength(1);
    expect(status.orders[0].product_option_id).toBe("premium_test_1_day_usd");
  }).toPass({ timeout: 10_000 });

  await markTestBoundary(testInfo, "END");
});

test("term toggle flips every tier card globally", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await setMockCentralScenario(request, {
    productOptionsResponse: productOptionsResponse([
      productTier(
        "basic",
        "Basic",
        10,
        1000,
        [
          productOption("basic_1_month_usd", 1, "month", "1 month", 500),
          productOption("basic_12_months_usd", 12, "month", "1 year", 5000, { presentation: { is_default: true } }),
        ],
        { is_featured: true, ribbon_label: "Early adopter discount" },
      ),
      productTier("premium", "Premium", 50, 50000, [
        productOption("premium_1_month_usd", 1, "month", "1 month", 5000),
        productOption("premium_12_months_usd", 12, "month", "1 year", 50000, { presentation: { is_default: true } }),
      ]),
    ]),
  });

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  // Initial defaults — yearly. Round amounts render without cents.
  await expect(page.getByTestId("payments-tier-price-basic")).toContainText("$50");
  await expect(page.getByTestId("payments-tier-price-basic")).toContainText("/ 1 year");
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("$500");
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("/ 1 year");

  // Click the "or $5 / 1 month" link on Basic — this flips BOTH cards
  // to monthly globally.
  await page.getByTestId("payments-term-toggle-basic_1_month_usd").click();
  await expect(page.getByTestId("payments-tier-price-basic")).toContainText("$5");
  await expect(page.getByTestId("payments-tier-price-basic")).toContainText("/ 1 month");
  await expect(page.getByTestId("payments-buy-btn-basic")).toContainText("$5");
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("$50");
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("/ 1 month");
  await expect(page.getByTestId("payments-buy-btn-premium")).toContainText("$50");

  // Clicking the under-CTA "Switch to yearly billing" link flips both back.
  await page.getByTestId("payments-switch-link-premium").click();
  await expect(page.getByTestId("payments-tier-price-basic")).toContainText("$50");
  await expect(page.getByTestId("payments-tier-price-basic")).toContainText("/ 1 year");
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("$500");

  await markTestBoundary(testInfo, "END");
});

test("order canceled via widget shows try-again and permits another checkout", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  await page.getByTestId("payments-buy-btn-premium").click();

  await expect(async () => {
    const calls = await page.evaluate(() => window.__atlosCalls ?? []);
    expect(calls.length).toBeGreaterThan(0);
  }).toPass({ timeout: 10_000 });

  await page.evaluate(() => window.__atlosCanceled?.());

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "canceled", { timeout: 5_000 });

  // Try again should launch a fresh ATLOS checkout from the same buy CTA.
  await page.getByTestId("payments-buy-btn-premium").click();
  await expect(async () => {
    const calls = await page.evaluate(() => window.__atlosCalls ?? []);
    expect(calls.length).toBeGreaterThanOrEqual(2);
  }).toPass({ timeout: 10_000 });

  await markTestBoundary(testInfo, "END");
});

test("failed order via check-now re-enables tier buy CTAs", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");
  await page.getByTestId("payments-buy-btn-premium").click();

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "pending", { timeout: 10_000 });
  const order = await getSingleMockCentralOrder(request);
  await expect(async () => {
    const status = await getMockCentralStatus(request);
    expect(status.orderStatusRequests?.[order.order_id] ?? 0).toBeGreaterThanOrEqual(1);
  }).toPass({ timeout: 10_000 });
  await expect(page.getByTestId("payments-check-now-btn")).toBeVisible();

  await setMockCentralScenario(request, { orderStatus: "failed" });

  await page.getByTestId("payments-check-now-btn").click();

  await expect(card).toHaveAttribute("data-status", "failed", { timeout: 10_000 });
  await expect(page.getByTestId("payments-buy-btn-premium")).toBeEnabled();

  await markTestBoundary(testInfo, "END");
});

test("confirmed payment shows summary and auto-advances to active", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");
  await page.getByTestId("payments-buy-btn-premium").click();

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "pending", { timeout: 10_000 });

  await setMockCentralScenario(request, {
    orderStatusResponse: {
      status: "pending",
      verification_state: "payment_confirmed_unverified",
      next_action: "keep_polling",
      payments: [
        {
          payment_id: "19A2D79298D2BC37A7D9569D8A",
          status: "confirmed",
          paid_order_amount: {
            minor_units: 999,
            currency: "USD",
            display_scale: 2,
          },
          paid_asset_amount: {
            amount: "0.00002614",
            asset_code: "XMR",
            blockchain_code: "XMR",
          },
          blockchain_hash: "d4331a38c1214af749eb5c12e7343156465fce70aa2df1c62bce626be0c58613",
          confirmed_at: "2026-04-21T19:11:45Z",
        },
      ],
    },
  });

  await page.getByTestId("payments-check-now-btn").click();

  await expect(card).toHaveAttribute("data-status", "verifying", { timeout: 10_000 });
  await expect(page.getByTestId("payments-summary")).toContainText("XMR");
  await expect(page.getByTestId("payments-summary")).toContainText("d4331a38");

  await setMockCentralScenario(request, {
    orderStatus: "paid",
    orderStatusResponse: null,
    paidTokenPayload: {
      premium_access_token: "test-token",
      token_id: "01JQABCDEF000000000000000F",
    },
  });

  await expect(card).toHaveAttribute("data-status", "active", { timeout: 35_000 });

  await markTestBoundary(testInfo, "END");
});

test("active premium shows support IDs and token-superseded warning", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");
  await page.getByTestId("payments-buy-btn-premium").click();
  const order = await getSingleMockCentralOrder(request);

  await setMockCentralScenario(request, {
    orderStatus: "paid",
    paidTokenPayload: {
      premium_access_token: "test-token",
      token_id: "01JQABCDEF000000000000000F",
    },
  });
  await page.evaluate(() => window.__atlosCompleted?.());

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "active", { timeout: 10_000 });
  const reference = page.getByTestId("payments-support-reference");
  await expect(reference).toContainText("Token ID");
  await expect(reference).toContainText("01JQABCDEF000000000000000F");
  await expect(reference).toContainText("Order ID");
  await expect(reference).toContainText(order.order_id);
  await expect(reference).toContainText("Subscription ID");
  await expect(reference).toContainText("01JQABCDEF000000000000000G");
  await expect(reference).toContainText("Entitlement holder ID");
  await expect(reference).not.toContainText("management_secret");
  await expect(reference).not.toContainText("premium_access_token");

  await setMockCentralScenario(request, {
    refreshOutcome: {
      status: "revoked",
      reason: "token_superseded",
    },
    historyOutcome: {
      orders: [],
      premium_access_token: null,
      token_id: null,
      subscription_valid_until: null,
      token_expires_at: null,
    },
  });

  await page.getByTestId("payments-refresh-btn").click();
  await expect(card).toHaveAttribute("data-status", "active_with_sync_warning", {
    timeout: 10_000,
  });
  await expect(page.getByTestId("payments-sync-warning")).toContainText("Central sync issue");
  await expect(page.getByTestId("payments-sync-warning")).toContainText("Retry sync");
  await expect(page.getByTestId("payments-support-reference")).toContainText(
    "01JQABCDEF000000000000000F",
  );
  await page.getByTestId("payments-sync-warning-dismiss-btn").click();
  await expect(page.getByTestId("payments-sync-warning")).toHaveCount(0);
  await page.getByTestId("payments-refresh-btn").click();
  await expect(page.getByTestId("payments-sync-warning")).toContainText("Central sync issue");

  await markTestBoundary(testInfo, "END");
});

test("underpayment top-up launches a new Atlos attempt for the parent order", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");
  await page.getByTestId("payments-buy-btn-premium").click();

  const mockOrder = await getSingleMockCentralOrder(request);
  const parentOrderId = mockOrder.order_id;
  const firstAttemptId = mockOrder.payment_attempt_id;
  const topUpAttemptId = "01JQABCDEF000000000000000G";

  await setMockCentralScenario(request, {
    orderStatusResponse: {
      status: "pending",
      verification_state: "additional_payment_required",
      next_action: "request_additional_payment",
      paid_amount_minor_units: 800,
      remaining_amount: {
        minor_units: 199,
        currency: "USD",
        display_scale: 2,
      },
      additional_payment_request: {
        payment_attempt_id: topUpAttemptId,
        provider: "atlos",
        merchant_id: "8MY8BXTU15",
        atlos_order_id: topUpAttemptId,
        amount: {
          minor_units: 199,
          currency: "USD",
          display_scale: 2,
        },
      },
      payments: [
        {
          payment_id: "19A2D79298D2BC37A7D9569D8A",
          payment_attempt_id: firstAttemptId,
          status: "confirmed",
          paid_order_amount: {
            minor_units: 800,
            currency: "USD",
            display_scale: 2,
          },
          paid_asset_amount: {
            amount: "0.00002000",
            asset_code: "XMR",
            blockchain_code: "XMR",
          },
          blockchain_hash: "d4331a38c1214af749eb5c12e7343156465fce70aa2df1c62bce626be0c58613",
          confirmed_at: "2026-04-21T19:11:45Z",
        },
      ],
    },
  });

  const card = page.getByTestId("payments-card");
  await page.getByTestId("payments-check-now-btn").click();
  await expect(card).toHaveAttribute("data-status", "additional_payment_required", {
    timeout: 10_000,
  });
  await expect(page.getByTestId("payments-top-up-summary")).toContainText("8.00 USD");
  await expect(page.getByTestId("payments-top-up-summary")).toContainText("1.99 USD");
  await expect(page.getByTestId("payments-summary")).toContainText(parentOrderId);

  await page.getByTestId("payments-top-up-btn").click();
  await expect(async () => {
    const calls = await page.evaluate(() => window.__atlosCalls ?? []);
    expect(calls).toHaveLength(2);
    expect(calls[1].orderId).toBe(topUpAttemptId);
    expect(calls[1].orderId).not.toBe(parentOrderId);
    expect(calls[1].orderAmount).toBe(1.99);
    expect(calls[1].orderCurrency).toBe("USD");
  }).toPass({ timeout: 10_000 });

  await setMockCentralScenario(request, {
    orderStatus: "paid",
    orderStatusResponse: null,
    paidTokenPayload: {
      premium_access_token: "test-token",
      token_id: "01JQABCDEF000000000000000F",
    },
  });
  await page.evaluate(() => window.__atlosCompleted?.());
  await expect(card).toHaveAttribute("data-status", "active", { timeout: 10_000 });

  await markTestBoundary(testInfo, "END");
});

test("additional-payment-required state survives reload with parent payment reference", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");
  await page.getByTestId("payments-buy-btn-premium").click();

  const mockOrder = await getSingleMockCentralOrder(request);
  const parentOrderId = mockOrder.order_id;

  await setMockCentralScenario(request, {
    orderStatusResponse: {
      status: "pending",
      verification_state: "additional_payment_required",
      next_action: "request_additional_payment",
      paid_amount_minor_units: 800,
      remaining_amount: {
        minor_units: 199,
        currency: "USD",
        display_scale: 2,
      },
      additional_payment_request: {
        payment_attempt_id: "01JQABCDEF000000000000000G",
        provider: "atlos",
        merchant_id: "8MY8BXTU15",
        atlos_order_id: "01JQABCDEF000000000000000G",
        amount: {
          minor_units: 199,
          currency: "USD",
          display_scale: 2,
        },
      },
      payments: [
        {
          payment_id: "19A2D79298D2BC37A7D9569D8A",
          status: "confirmed",
          paid_order_amount: {
            minor_units: 800,
            currency: "USD",
            display_scale: 2,
          },
        },
      ],
    },
  });

  await page.reload();
  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "additional_payment_required", {
    timeout: 10_000,
  });
  await expect(page.getByTestId("payments-top-up-summary")).toContainText("8.00 USD");
  await expect(page.getByTestId("payments-top-up-summary")).toContainText("1.99 USD");
  await expect(page.getByTestId("payments-summary")).toContainText(parentOrderId);
  await expect(page.getByTestId("payments-top-up-btn")).toBeVisible();

  await markTestBoundary(testInfo, "END");
});

test("manual review survives reload and stops automatic retry-oriented flow", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");
  await page.getByTestId("payments-buy-btn-premium").click();

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "pending", { timeout: 10_000 });

  await setMockCentralScenario(request, {
    orderStatusResponse: {
      status: "expired",
      verification_state: "under_manual_review",
      next_action: "show_manual_review",
      manual_review: {
        reason: "amount_mismatch",
        resolved: false,
      },
      payments: [
        {
          payment_id: "19A2D79298D2BC37A7D9569D8A",
          status: "confirmed",
          paid_order_amount: {
            minor_units: 800,
            currency: "USD",
            display_scale: 2,
          },
          paid_asset_amount: {
            amount: "0.00002614",
            asset_code: "XMR",
            blockchain_code: "XMR",
          },
          blockchain_hash: "d4331a38c1214af749eb5c12e7343156465fce70aa2df1c62bce626be0c58613",
          confirmed_at: "2026-04-21T19:11:45Z",
        },
      ],
    },
  });

  await page
    .getByTestId("payments-check-now-btn")
    .click({ timeout: 10_000 })
    .catch(async (error) => {
      if ((await card.getAttribute("data-status")) !== "manual_review") {
        throw error;
      }
    });

  await expect(card).toHaveAttribute("data-status", "manual_review", { timeout: 10_000 });
  await expect(page.getByText("Payment needs review")).toBeVisible();
  await expect(page.getByTestId("payments-summary")).toContainText("XMR");
  await expect(page.getByTestId("payments-summary")).toContainText("d4331a38");

  await page.reload();
  await expect(card).toHaveAttribute("data-status", "manual_review", { timeout: 10_000 });
  await expect(page.getByTestId("payments-check-later-btn")).toBeVisible();

  await markTestBoundary(testInfo, "END");
});

test("unsupported signing key blocks ATLOS launch", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await setMockCentralScenario(request, { signingKeyUnsupported: true });

  await page.goto("/payments");
  // With no tier grid, the upgrade-required path renders instead of buy buttons.
  await expect(page.getByTestId("payments-update-required-notice")).toBeVisible();

  // ATLOS must not have been invoked when Central rejects the signing key.
  await page.waitForTimeout(1_500);
  const calls = await page.evaluate(() => window.__atlosCalls ?? []);
  expect(calls).toEqual([]);

  // UI should not be in an "active" state.
  const card = page.getByTestId("payments-card");
  const status = await card.getAttribute("data-status");
  expect(status).not.toBe("active");

  await markTestBoundary(testInfo, "END");
});

test("missing product options disable checkout without changing local status", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await setMockCentralScenario(request, { productOptionsUnavailable: true });

  await page.goto("/payments");

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "not_active");
  await expect(page.getByTestId("payments-catalog-error")).toContainText(
    "Price unavailable",
  );
  await expect(page.getByTestId("payments-tier-grid")).toHaveCount(0);

  const calls = await page.evaluate(() => window.__atlosCalls ?? []);
  expect(calls).toEqual([]);

  await markTestBoundary(testInfo, "END");
});

test("no usable premium options disable checkout", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await setMockCentralScenario(request, {
    productOptionsResponse: productOptionsResponse([
      productTier("business", "Business", 100, 100000, [
        productOption("business_12_months_usd", 12, "month", "1 year", 123),
      ]),
      productTier("premium", "Premium", 50, 50000, [
        {
          id: "premium_12_months_usd",
          term: {
            label: "1 year",
          },
          price: {
            minor_units: 123,
            currency: "USD",
            display_scale: 2,
          },
        },
      ]),
    ]),
  });

  await page.goto("/payments");

  // No valid premium purchase option survives parsing → no buy CTA renders
  // on Premium (the Unavailable placeholder is intentionally absent).
  await expect(page.getByTestId("payments-buy-btn-premium")).toHaveCount(0);

  const calls = await page.evaluate(() => window.__atlosCalls ?? []);
  expect(calls).toEqual([]);

  await markTestBoundary(testInfo, "END");
});

test("upgrade-required disables every tier buy CTA and shows update notice", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await setMockCentralScenario(request, { productOptionsUpgradeRequired: true });

  await page.goto("/payments");

  await expect(page.getByTestId("payments-update-required-notice")).toBeVisible();
  await expect(page.getByTestId("payments-tier-price-premium")).toContainText("$500");
  await expect(page.getByTestId("payments-buy-btn-premium")).toBeDisabled();

  const calls = await page.evaluate(() => window.__atlosCalls ?? []);
  expect(calls).toEqual([]);

  await markTestBoundary(testInfo, "END");
});

test("history reconciliation after cancel finds paid and activates premium", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  await page.getByTestId("payments-buy-btn-premium").click();

  await latestAtlosOrderId(page);
  const orderId = (await getSingleMockCentralOrder(request)).order_id;

  await page.evaluate(() => window.__atlosCanceled?.());

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "canceled", { timeout: 5_000 });

  // Set history to report this order as paid with a valid token payload.
  const paidAt = new Date().toISOString();
  const subscriptionValidUntil = new Date(
    Date.now() + 365 * 24 * 60 * 60 * 1000,
  ).toISOString();
  const tokenExpiresAt = new Date(Date.now() + 7 * 24 * 60 * 60 * 1000).toISOString();
  await setMockCentralScenario(request, {
    historyOutcome: {
      orders: [{ order_id: orderId, status: "paid", paid_at: paidAt }],
      premium_access_token: "test-token",
      token_id: "01JQABCDEF000000000000000F",
      subscription_valid_until: subscriptionValidUntil,
      token_expires_at: tokenExpiresAt,
    },
  });

  // Reload triggers auto-reconcile which should find the paid order.
  await page.reload();
  await expect(card).toHaveAttribute("data-status", "active", { timeout: 10_000 });

  await markTestBoundary(testInfo, "END");
});

test("paid-plan terms sit directly under the plan cards", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  const card = page.getByTestId("payments-card");
  await expect(card).toBeVisible();
  await expect(page.getByTestId("payments-paid-plan-terms")).toBeVisible();

  // Terms must precede the commitments block in the DOM (adjacent to cards,
  // not detached below the editorial commitments).
  const order = await card.evaluate((el) => {
    const terms = el.querySelector("[data-testid='payments-paid-plan-terms']");
    const commitments = el.querySelector(".payments-commitments");
    if (!terms || !commitments) return "missing";
    const rel = terms.compareDocumentPosition(commitments);
    return rel & Node.DOCUMENT_POSITION_FOLLOWING ? "terms-first" : "commitments-first";
  });
  expect(order).toBe("terms-first");
});

test("sidebar shows Upgrade and /payments still loads", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);

  const upgradeLink = page.getByTestId("sidebar-link-upgrade");
  await expect(upgradeLink).toContainText("Upgrade");
  await expect(upgradeLink).not.toContainText("Payments");

  await upgradeLink.click();
  await expect(page).toHaveURL(/\/payments$/);
  await expect(page.getByTestId("payments-card")).toBeVisible();
});

test("history reconciliation with no stronger outcome leaves Buy CTA enabled", async ({
  page,
  request,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  await page.getByTestId("payments-buy-btn-premium").click();

  await latestAtlosOrderId(page);
  const orderId = (await getSingleMockCentralOrder(request)).order_id;

  await page.evaluate(() => window.__atlosCanceled?.());

  const card = page.getByTestId("payments-card");
  await expect(card).toHaveAttribute("data-status", "canceled", { timeout: 5_000 });

  // History reports order still pending - no stronger outcome.
  await setMockCentralScenario(request, {
    historyOutcome: {
      orders: [{ order_id: orderId, status: "pending" }],
      premium_access_token: null,
      token_id: null,
      subscription_valid_until: null,
      token_expires_at: null,
    },
  });

  await page.reload();
  await expect(card).toHaveAttribute("data-status", "canceled", { timeout: 10_000 });
  await expect(page.getByTestId("payments-buy-btn-premium")).toBeEnabled();

  await markTestBoundary(testInfo, "END");
});

test("intro copy makes no unlimited promise", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  // Scope to the app-authored intro paragraph. Tier bullets are Central-owned
  // catalog copy (cleaned separately on the Central side); the app must not
  // hard-code an "unlimited" promise of its own.
  const intro = page.getByTestId("payments-card").locator(".payments-intro");
  await expect(intro).toBeVisible();
  await expect(intro).not.toContainText(/unlimited/i);
});

test("central pricing summary renders as intro chips and free tier prices $0", async ({
  page,
  request,
}) => {
  // Garden-renamed tiers: the intro must still read correctly and the free
  // tier must still show its cost even when no tier is literally named "Free".
  await setMockCentralScenario(request, {
    productOptionsResponse: productOptionsResponse(
      [
        productTier("free", "Sprout", 50, 0, []),
        productTier("basic", "Harvest", 200, 3000, [
          productOption("basic_12_months_usd", 12, "month", "1 year", 4995, {
            presentation: { is_default: true },
          }),
        ]),
      ],
      {
        pricing_summary:
          "**Sprout** tracks what you hold. **Harvest** does the accounting.",
      },
    ),
  });

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  const intro = page.getByTestId("payments-card").locator(".payments-intro");
  await expect(intro).toContainText(
    "Sprout tracks what you hold. Harvest does the accounting.",
  );
  // Bold runs become the bordered tier chips.
  await expect(intro.locator("em").nth(0)).toHaveText("Sprout");
  await expect(intro.locator("em").nth(1)).toHaveText("Harvest");

  const freePrice = page.getByTestId("payments-tier-price-free");
  await expect(freePrice).toContainText("$0");
  await expect(freePrice).toContainText("free");
});

test("yearly-only catalog renders no toggle and trims whole-dollar cents", async ({
  page,
  request,
}) => {
  await setMockCentralScenario(request, {
    productOptionsResponse: productOptionsResponse([
      productTier("premium", "Premium", 50, 50000, [
        productOption("premium_12_months_usd", 12, "month", "1 year", 9900, {
          presentation: { is_default: true },
        }),
      ]),
    ]),
  });

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/payments");

  const price = page.getByTestId("payments-tier-price-premium");
  await expect(price).toContainText("$99");
  await expect(price).toContainText("/ 1 year");
  // Whole-dollar amount drops the cents.
  await expect(price).not.toContainText("$99.00");

  // No alternate term ⇒ no toggle anywhere on the page.
  await expect(page.locator("[data-testid^='payments-term-toggle-']")).toHaveCount(0);
});
