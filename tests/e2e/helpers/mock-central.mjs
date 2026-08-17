import { createServer } from "node:http";
import crypto from "node:crypto";

const DEFAULT_MERCHANT_ID = "8MY8BXTU15";
const DEFAULT_ORDER_SECRET = "frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI";
const DEFAULT_MANAGEMENT_SECRET = "5FuYMBR_MhwubKAJQeNMrUH0JD3PvFuyt3sfFh0ezLw";
const DEFAULT_TOKEN_ID = "01JQABCDEF000000000000000F";
const DEFAULT_SUBSCRIPTION_SUBJECT_ID = "01JQABCDEF000000000000000G";
const TEST_SIGNING_PRIVATE_KEY = crypto.createPrivateKey({
  key: Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"),
    Buffer.alloc(32),
  ]),
  format: "der",
  type: "pkcs8",
});

function generateOrderId() {
  const chars = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let out = chars[Math.floor(Math.random() * 8)];
  for (let i = 1; i < 26; i += 1) {
    out += chars[Math.floor(Math.random() * chars.length)];
  }
  return out;
}

function readJsonBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      const raw = Buffer.concat(chunks).toString("utf8");
      if (raw.length === 0) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(raw));
      } catch (error) {
        reject(error);
      }
    });
    req.on("error", reject);
  });
}

function writeJson(res, status, body) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

function signPremiumToken(claims) {
  const claimsJson = Buffer.from(JSON.stringify(claims), "utf8");
  const signature = crypto.sign(null, claimsJson, TEST_SIGNING_PRIVATE_KEY);
  return `${claimsJson.toString("base64url")}.${signature.toString("base64url")}`;
}

function capabilitiesForTier(tier) {
  if (tier === "basic") {
    return {
      limits: {
        accounts: { total: 10 },
        synced_accounts: 10,
        history: { max_transactions_per_account: 10000 },
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
  }

  if (tier === "premium") {
    return {
      limits: {
        accounts: { total: 50 },
        synced_accounts: 50,
        history: { max_transactions_per_account: 100000 },
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
  }

  return {
    limits: {
      accounts: { total: 20 },
      history: { max_transactions_per_account: 0 },
    },
    features: {
      balance_assertions: false,
      balance_sync: true,
      exchange_rates_current: true,
      exchange_rates_history: false,
      hledger_export: false,
      price_overrides: false,
      tax_reports: false,
      transaction_history_sync: false,
    },
  };
}

function tierForOrder(order, payload) {
  if (payload?.tier) {
    return payload.tier;
  }
  return order?.product_option_id?.startsWith("basic") ? "basic" : "premium";
}

function capabilitySetIdForTier(tier) {
  return `${tier}.v3`;
}

function tokenPayloadForOrder(order, payload) {
  if (!order || !payload || payload.premium_access_token !== "test-token") {
    return payload;
  }
  const tier = tierForOrder(order, payload);
  const paidAt = payload.paid_at ?? new Date().toISOString();
  const subscriptionValidUntil =
    payload.subscription_valid_until ??
    new Date(Date.now() + 365 * 24 * 60 * 60 * 1000).toISOString();
  const tokenExpiresAt =
    payload.token_expires_at ?? new Date(Date.now() + 7 * 24 * 60 * 60 * 1000).toISOString();
  const tokenId = payload.token_id ?? DEFAULT_TOKEN_ID;
  return {
    ...payload,
    token_id: tokenId,
    paid_at: paidAt,
    subscription_valid_until: subscriptionValidUntil,
    token_expires_at: tokenExpiresAt,
    premium_access_token: signPremiumToken({
      token_id: tokenId,
      subscription_subject_id: DEFAULT_SUBSCRIPTION_SUBJECT_ID,
      entitlement_holder_id: order.entitlement_holder_id,
      tier,
      capability_set_id: capabilitySetIdForTier(tier),
      capability_schema_version: payload.capability_schema_version ?? 3,
      capabilities: payload.capabilities ?? capabilitiesForTier(tier),
      subscription_valid_until: subscriptionValidUntil,
      token_expires_at: tokenExpiresAt,
      issued_at: new Date(Date.now() - 60_000).toISOString(),
    }),
  };
}

function tokenPayloadForTransfer(payload) {
  const tier = payload.tier ?? "premium";
  const subscriptionValidUntil =
    payload.subscription_valid_until ??
    new Date(Date.now() + 365 * 24 * 60 * 60 * 1000).toISOString();
  const tokenExpiresAt =
    payload.token_expires_at ?? new Date(Date.now() + 7 * 24 * 60 * 60 * 1000).toISOString();
  const tokenId = payload.token_id ?? DEFAULT_TOKEN_ID;
  return {
    status: "active",
    ...payload,
    token_id: tokenId,
    subscription_valid_until: subscriptionValidUntil,
    token_expires_at: tokenExpiresAt,
    premium_access_token: signPremiumToken({
      token_id: tokenId,
      subscription_subject_id: DEFAULT_SUBSCRIPTION_SUBJECT_ID,
      entitlement_holder_id: payload.new_entitlement_holder_id,
      tier,
      capability_set_id: capabilitySetIdForTier(tier),
      capability_schema_version: payload.capability_schema_version ?? 3,
      capabilities: payload.capabilities ?? capabilitiesForTier(tier),
      subscription_valid_until: subscriptionValidUntil,
      token_expires_at: tokenExpiresAt,
      issued_at: new Date(Date.now() - 60_000).toISOString(),
    }),
  };
}

function orderStatusResponse(status, extra = {}) {
  const base = {
    premium_granted: false,
    payments: [],
  };

  if (status === "pending") {
    return {
      ...base,
      status: "pending",
      verification_state: "awaiting_payment",
      next_action: "keep_polling",
      ...extra,
    };
  }

  if (status === "expired" || status === "failed") {
    return {
      ...base,
      status,
      verification_state: "awaiting_payment",
      next_action: "offer_retry",
      ...extra,
    };
  }

  if (status === "paid") {
    return {
      ...base,
      status: "paid",
      verification_state: "premium_granted",
      premium_granted: true,
      next_action: "unlock_premium",
      ...extra,
    };
  }

  return {
    ...base,
    status,
    ...extra,
  };
}

function defaultProductOptionsResponse() {
  return {
    catalog_schema_version: 4,
    tiers: [
      {
        tier: "free",
        display_name: "Free",
        presentation: {
          display_order: 10,
          summary:
            "Local ownership and twenty balance-only synced accounts.",
          bullets: [
            "**20** balance-synced accounts",
            "Balance-only sync — no transaction history",
            "Unlimited unsynced accounts & custom assets",
          ],
          is_featured: false,
          ribbon_label: null,
        },
        capability_set_id: "free.v3",
        capability_schema_version: 3,
        minimum_app_version: null,
        capabilities: {
          limits: {
            accounts: {
              total: 20,
            },
            history: {
              max_transactions_per_account: 0,
            },
          },
          features: {
            balance_assertions: false,
            balance_sync: true,
            exchange_rates_current: true,
            exchange_rates_history: false,
            hledger_export: false,
            price_overrides: false,
            tax_reports: false,
            transaction_history_sync: false,
          },
        },
        purchase_options: [],
      },
      {
        tier: "basic",
        display_name: "Basic",
        presentation: {
          display_order: 20,
          summary: "Ten synced accounts with full transaction history up to 10,000 transactions each.",
          bullets: [
            "**10** synced accounts",
            "Full transaction history up to 10,000 / account",
            "Unlimited unsynced accounts & custom assets",
            "hledger & ledger-cli export, fully unlocked",
          ],
          is_featured: true,
          ribbon_label: "Early adopter discount",
        },
        capability_set_id: "basic.v3",
        capability_schema_version: 3,
        minimum_app_version: null,
        capabilities: {
          limits: {
            accounts: {
              total: 10,
            },
            synced_accounts: 10,
            history: {
              max_transactions_per_account: 10000,
            },
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
        },
        purchase_options: [
          {
            id: "basic_1_month_usd",
            term: {
              quantity: 1,
              unit: "month",
              label: "1 month",
            },
            price: {
              minor_units: 500,
              currency: "USD",
              currency_symbol: "$",
              display_scale: 2,
            },
            presentation: {
              display_order: 10,
            },
          },
          {
            id: "basic_12_months_usd",
            term: {
              quantity: 12,
              unit: "month",
              label: "1 year",
            },
            price: {
              minor_units: 5000,
              currency: "USD",
              currency_symbol: "$",
              display_scale: 2,
            },
            presentation: {
              display_order: 20,
              is_default: true,
              badge: "Best value",
            },
          },
        ],
      },
      {
        tier: "premium",
        display_name: "Premium",
        presentation: {
          display_order: 30,
          summary:
            "Fifty synced accounts, deep histories up to 100,000 transactions each, plus future advanced automation.",
          bullets: [
            "**50** synced accounts",
            "Full transaction history up to 100,000 / account",
            "Unlimited unsynced accounts & custom assets",
            "hledger & ledger-cli export, fully unlocked",
          ],
          is_featured: false,
          ribbon_label: null,
        },
        capability_set_id: "premium.v3",
        capability_schema_version: 3,
        minimum_app_version: null,
        capabilities: {
          limits: {
            accounts: {
              total: 50,
            },
            synced_accounts: 50,
            history: {
              max_transactions_per_account: 100000,
            },
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
        },
        purchase_options: [
          {
            id: "premium_1_month_usd",
            term: {
              quantity: 1,
              unit: "month",
              label: "1 month",
            },
            price: {
              minor_units: 5000,
              currency: "USD",
              currency_symbol: "$",
              display_scale: 2,
            },
            presentation: {
              display_order: 10,
            },
          },
          {
            id: "premium_12_months_usd",
            term: {
              quantity: 12,
              unit: "month",
              label: "1 year",
            },
            price: {
              minor_units: 50000,
              currency: "USD",
              currency_symbol: "$",
              display_scale: 2,
            },
            presentation: {
              display_order: 20,
              badge: "Premium",
            },
          },
        ],
      },
    ],
  };
}

function currentProductOptionsResponse(scenario) {
  if (scenario.productOptionsResponse) {
    return structuredClone(scenario.productOptionsResponse);
  }
  const response = defaultProductOptionsResponse();
  if (scenario.productOptionsUpgradeRequired) {
    response.app_compatibility = {
      status: "upgrade_required",
      detail: "BitGarth needs an update before Premium can be purchased or refreshed.",
      minimum_app_version: null,
    };
  }
  return response;
}

export async function startMockCentralServer({ port = 0 } = {}) {
  const scenario = {
    signingKeyUnsupported: false,
    orderStatus: "pending",
    orderStatusResponse: null,
    paidTokenPayload: null,
    refreshOutcome: null,
    historyOutcome: null,
    transferOutcome: "active",
    transferResponse: null,
    productOptionsUnavailable: false,
    productOptionsUpgradeRequired: false,
    productOptionsResponse: null,
    latestAppVersion: {
      latest: "9.9.9",
      image: "bitgarth/bitgarth:9.9.9",
      release_url: "https://hub.docker.com/r/bitgarth/bitgarth/tags?name=9.9.9",
      published_at: "2026-06-07T12:00:00Z",
    },
  };

  const orders = new Map();
  const orderStatusRequests = new Map();

  function setScenario(patch) {
    Object.assign(scenario, patch);
  }

  function reset() {
    scenario.signingKeyUnsupported = false;
    scenario.orderStatus = "pending";
    scenario.orderStatusResponse = null;
    scenario.paidTokenPayload = null;
    scenario.refreshOutcome = null;
    scenario.historyOutcome = null;
    scenario.transferOutcome = "active";
    scenario.transferResponse = null;
    scenario.productOptionsUnavailable = false;
    scenario.productOptionsUpgradeRequired = false;
    scenario.productOptionsResponse = null;
    scenario.latestAppVersion = {
      latest: "9.9.9",
      image: "bitgarth/bitgarth:9.9.9",
      release_url: "https://hub.docker.com/r/bitgarth/bitgarth/tags?name=9.9.9",
      published_at: "2026-06-07T12:00:00Z",
    };
    orders.clear();
    orderStatusRequests.clear();
  }

  function signingKeyGate(res) {
    if (!scenario.signingKeyUnsupported) {
      return false;
    }
    writeJson(res, 426, {
      error_code: "unsupported_signing_key",
      message: "Signing key is not supported by this Central deployment",
    });
    return true;
  }

  const server = createServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? "/", "http://127.0.0.1");

      if (req.method === "POST" && url.pathname === "/__mock/scenario") {
        const body = await readJsonBody(req);
        setScenario(body);
        writeJson(res, 200, { ok: true });
        return;
      }

      if (req.method === "POST" && url.pathname === "/__mock/reset") {
        reset();
        writeJson(res, 200, { ok: true });
        return;
      }

      if (req.method === "GET" && url.pathname === "/__mock/status") {
        writeJson(res, 200, {
          scenario,
          orders: Array.from(orders.entries()).map(([order_id, order]) => ({
            order_id,
            ...order,
          })),
          orderStatusRequests: Object.fromEntries(orderStatusRequests.entries()),
        });
        return;
      }

      if (req.method === "GET" && url.pathname === "/api/v1/latest-app-version") {
        const channel = req.headers["x-bitgarth-app-channel"];
        if (!channel) {
          writeJson(res, 400, {
            error_code: "missing_app_channel",
            message: "x-bitgarth-app-channel is required",
          });
          return;
        }
        if (channel !== "docker") {
          writeJson(res, 422, {
            error_code: "unsupported_app_channel",
            message: "latest app version is only available for docker",
          });
          return;
        }
        writeJson(res, 200, scenario.latestAppVersion);
        return;
      }

      if (
        req.method === "GET" &&
        url.pathname === "/api/v1/payments/product-options"
      ) {
        if (signingKeyGate(res)) return;
        if (scenario.productOptionsUnavailable) {
          writeJson(res, 503, {
            error_code: "unavailable",
            message: "product options unavailable",
          });
          return;
        }
        const response = currentProductOptionsResponse(scenario);
        writeJson(res, 200, response);
        return;
      }

      if (
        req.method === "POST" &&
        url.pathname === "/api/v1/payments/orders/session"
      ) {
        if (signingKeyGate(res)) return;
        const body = await readJsonBody(req);
        const selectedOption = (currentProductOptionsResponse(scenario).tiers ?? [])
          .flatMap((tier) => tier.purchase_options ?? [])
          .find((option) => option.id === body.product_option_id);
        if (!body.entitlement_holder_id || !selectedOption) {
          writeJson(res, 400, { error_code: "bad_request", message: "invalid body" });
          return;
        }
        const hasBearer = (req.headers["authorization"] ?? "").startsWith("Bearer ");
        const order_id = generateOrderId();
        const payment_attempt_id = generateOrderId();
        const order_amount = {
          minor_units: selectedOption.price.minor_units,
          currency: selectedOption.price.currency,
          currency_symbol: selectedOption.price.currency_symbol,
          display_scale: selectedOption.price.display_scale,
        };
        orders.set(order_id, {
          created_at: Date.now(),
          entitlement_holder_id: body.entitlement_holder_id,
          product_option_id: body.product_option_id,
          payment_attempt_id,
        });
        const response = {
          order_id,
          product_option_id: body.product_option_id,
          order_secret: DEFAULT_ORDER_SECRET,
          merchant_id: DEFAULT_MERCHANT_ID,
          order_amount,
          payment_attempt: {
            payment_attempt_id,
            provider: "atlos",
            atlos_order_id: payment_attempt_id,
            amount: order_amount,
          },
        };
        if (!hasBearer) {
          response.management_secret = DEFAULT_MANAGEMENT_SECRET;
        }
        writeJson(res, 200, response);
        return;
      }

      const statusMatch = url.pathname.match(
        /^\/api\/v1\/payments\/orders\/([^/]+)\/status$/,
      );
      if (req.method === "GET" && statusMatch) {
        if (signingKeyGate(res)) return;
        const [, orderId] = statusMatch;
        if (!orders.has(orderId)) {
          writeJson(res, 404, { error_code: "not_found", message: "unknown order" });
          return;
        }
        orderStatusRequests.set(orderId, (orderStatusRequests.get(orderId) ?? 0) + 1);
        if (scenario.orderStatusResponse) {
          const payload =
            scenario.orderStatusResponse.status === "paid"
              ? tokenPayloadForOrder(orders.get(orderId), scenario.orderStatusResponse)
              : scenario.orderStatusResponse;
          writeJson(
            res,
            200,
            orderStatusResponse(
              scenario.orderStatusResponse.status ?? scenario.orderStatus,
              payload,
            ),
          );
          return;
        }
        if (scenario.orderStatus === "paid" && scenario.paidTokenPayload) {
          writeJson(
            res,
            200,
            orderStatusResponse(
              "paid",
              tokenPayloadForOrder(orders.get(orderId), scenario.paidTokenPayload),
            ),
          );
          return;
        }
        if (scenario.orderStatus === "paid") {
          writeJson(res, 500, {
            error_code: "internal",
            message: "scenario missing paidTokenPayload",
          });
          return;
        }
        writeJson(res, 200, orderStatusResponse(scenario.orderStatus));
        return;
      }

      if (req.method === "POST" && url.pathname === "/api/v1/payments/subscription/refresh") {
        if (signingKeyGate(res)) return;
        if (!scenario.refreshOutcome) {
          writeJson(res, 200, { status: "revoked", reason: "expired" });
          return;
        }
        writeJson(res, 200, scenario.refreshOutcome);
        return;
      }

      if (req.method === "POST" && url.pathname === "/api/v1/payments/subscription/transfer") {
        if (signingKeyGate(res)) return;
        const body = await readJsonBody(req);
        if (!body.new_entitlement_holder_id || !body.new_management_secret) {
          writeJson(res, 400, { error_code: "bad_request", message: "invalid body" });
          return;
        }
        if (scenario.transferOutcome === "service_unavailable") {
          writeJson(res, 503, {
            error_code: "service_unavailable",
            message: "try again",
          });
          return;
        }
        if (
          scenario.transferOutcome === "invalid_management_secret" ||
          req.headers.authorization !== `Bearer ${DEFAULT_MANAGEMENT_SECRET}`
        ) {
          writeJson(res, 401, {
            error_code: "invalid_management_secret",
            message: "invalid management secret",
          });
          return;
        }
        writeJson(
          res,
          200,
          tokenPayloadForTransfer({
            ...(scenario.transferResponse ?? {}),
            new_entitlement_holder_id: body.new_entitlement_holder_id,
          }),
        );
        return;
      }

      if (req.method === "GET" && url.pathname === "/api/v1/payments/subscription/history") {
        if (signingKeyGate(res)) return;
        if (!scenario.historyOutcome) {
          writeJson(res, 200, {
            orders: [],
            premium_access_token: null,
            token_id: null,
            subscription_valid_until: null,
            token_expires_at: null,
          });
          return;
        }
        const paidOrderId = scenario.historyOutcome.orders?.find(
          (order) => order.status === "paid",
        )?.order_id;
        writeJson(res, 200, {
          ...scenario.historyOutcome,
          ...tokenPayloadForOrder(orders.get(paidOrderId), scenario.historyOutcome),
        });
        return;
      }

      writeJson(res, 404, { error_code: "not_found", message: "unknown route" });
    } catch (error) {
      writeJson(res, 500, { error_code: "internal", message: String(error) });
    }
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolve);
  });

  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("Mock Central server failed to bind to an IPv4 socket");
  }

  const baseUrl = `http://127.0.0.1:${address.port}`;

  async function close() {
    await new Promise((resolve, reject) => {
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }
        resolve();
      });
    });
  }

  return {
    baseUrl,
    port: address.port,
    close,
    setScenario,
    reset,
  };
}
