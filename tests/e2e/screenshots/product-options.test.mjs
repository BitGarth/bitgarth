import assert from "node:assert/strict";
import test from "node:test";

import {
  EXPECTED_SIGNING_KEY_HASH_HEADER,
  PRODUCTION_EXPECTED_SIGNING_KEY_HASH,
  PRODUCTION_PRODUCT_OPTIONS_URL,
  fetchProductionProductOptions,
  seedMockCentralProductOptions,
} from "./product-options.mjs";

function minimalProductOptionsResponse() {
  return {
    catalog_schema_version: 4,
    tiers: [
      {
        tier: "free",
        display_name: "Free",
        capability_schema_version: 3,
        capabilities: {
          limits: {
            accounts: { total: 20 },
            synced_accounts: 20,
            history: { max_transactions_per_account: 0 },
          },
        },
        presentation: {
          summary: "Free screenshot tier",
          bullets: [],
          is_featured: false,
          ribbon_label: null,
        },
        purchase_options: [],
      },
      {
        tier: "basic",
        display_name: "Basic",
        capabilities: {
          limits: {
            synced_accounts: 10,
            history: { max_transactions_per_account: 1000 },
          },
        },
        presentation: {
          summary: "Basic screenshot tier",
        },
        purchase_options: [
          {
            id: "basic_12_months_usd",
            term: { quantity: 12, unit: "month", label: "1 year" },
            price: {
              minor_units: 5000,
              currency: "USD",
              currency_symbol: "$",
              display_scale: 2,
            },
          },
        ],
      },
    ],
  };
}

function minimalPremiumTier() {
  return {
    tier: "premium",
    display_name: "Premium",
    capabilities: {
      limits: {
        synced_accounts: 20,
        history: { max_transactions_per_account: 1000 },
      },
    },
    presentation: {
      summary: "Premium screenshot tier",
    },
    purchase_options: [
      {
        id: "premium_12_months_usd",
        term: { quantity: 12, unit: "month", label: "1 year" },
        price: {
          minor_units: 9000,
          currency: "USD",
          currency_symbol: "$",
          display_scale: 2,
        },
      },
    ],
  };
}

function tierByName(body, tierName) {
  return body.tiers.find((tier) => tier.tier === tierName);
}

async function assertRejectsProductOptionsBody(body, pattern) {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return body;
      },
    })),
    pattern,
  );
}

test("fetchProductionProductOptions sends production URL and signing-key hash header", async () => {
  let observedUrl = null;
  let observedOptions = null;
  const body = minimalProductOptionsResponse();

  const result = await fetchProductionProductOptions(async (url, options) => {
    observedUrl = url;
    observedOptions = options;
    return {
      ok: true,
      status: 200,
      async json() {
        return body;
      },
    };
  });

  assert.equal(observedUrl, PRODUCTION_PRODUCT_OPTIONS_URL);
  assert.equal(
    observedOptions.headers[EXPECTED_SIGNING_KEY_HASH_HEADER],
    PRODUCTION_EXPECTED_SIGNING_KEY_HASH,
  );
  assert.equal(result, body);
});

test("fetchProductionProductOptions rejects network failures", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => {
      throw new Error("connection refused");
    }),
    /Failed to fetch production product-options: connection refused/,
  );
});

test("fetchProductionProductOptions rejects non-2xx responses", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: false,
      status: 503,
      async json() {
        return { error_code: "unavailable" };
      },
    })),
    /Production product-options request failed: HTTP 503/,
  );
});

test("fetchProductionProductOptions rejects invalid JSON", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        throw new Error("Unexpected token");
      },
    })),
    /Production product-options response was not valid JSON: Unexpected token/,
  );
});

test("fetchProductionProductOptions rejects non-object responses", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return null;
      },
    })),
    /Production product-options response must be a JSON object/,
  );
});

test("fetchProductionProductOptions rejects missing tiers array", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return { catalog_schema_version: 4 };
      },
    })),
    /Production product-options response must contain a tiers array/,
  );
});

test("fetchProductionProductOptions rejects empty tiers array", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return { catalog_schema_version: 4, tiers: [] };
      },
    })),
    /Production product-options response must contain at least one tier/,
  );
});

test("fetchProductionProductOptions rejects missing or low catalog schema versions", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return { ...minimalProductOptionsResponse(), catalog_schema_version: 3 };
      },
    })),
    /Production product-options response must contain catalog_schema_version >= 4/,
  );
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        const { catalog_schema_version: _version, ...body } =
          minimalProductOptionsResponse();
        return body;
      },
    })),
    /Production product-options response must contain catalog_schema_version >= 4/,
  );
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return { ...minimalProductOptionsResponse(), catalog_schema_version: 4.5 };
      },
    })),
    /Production product-options response must contain catalog_schema_version >= 4/,
  );
});

test("fetchProductionProductOptions rejects catalogs without purchase options", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return {
          catalog_schema_version: 4,
          tiers: [{ tier: "basic", purchase_options: [] }],
        };
      },
    })),
    /Production product-options response must contain at least one purchase option/,
  );
});

test("fetchProductionProductOptions rejects purchase options without usable pricing", async () => {
  const body = minimalProductOptionsResponse();
  delete tierByName(body, "basic").purchase_options[0].price;

  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return body;
      },
    })),
    /Production product-options response contains an unusable purchase option/,
  );
});

test("fetchProductionProductOptions rejects tiers missing app-renderable fields", async () => {
  const missingSummary = minimalProductOptionsResponse();
  delete missingSummary.tiers[0].presentation.summary;

  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return missingSummary;
      },
    })),
    /Production product-options response contains an unusable tier/,
  );

  const missingHistoryLimit = minimalProductOptionsResponse();
  delete missingHistoryLimit.tiers[0].capabilities.limits.history
    .max_transactions_per_account;

  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return missingHistoryLimit;
      },
    })),
    /Production product-options response contains an unusable tier/,
  );
});

test("fetchProductionProductOptions rejects malformed premium tiers in otherwise usable catalogs", async () => {
  const body = minimalProductOptionsResponse();
  const premiumTier = minimalPremiumTier();
  delete premiumTier.presentation.summary;
  body.tiers.push(premiumTier);

  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return body;
      },
    })),
    /Production product-options response contains an unusable tier/,
  );
});

test("fetchProductionProductOptions rejects whitespace-only presentation summary", async () => {
  const body = minimalProductOptionsResponse();
  tierByName(body, "basic").presentation.summary = "   ";

  await assertRejectsProductOptionsBody(
    body,
    /Production product-options response contains an unusable tier/,
  );
});

test("fetchProductionProductOptions rejects malformed presentation field types", async () => {
  const malformedBullets = minimalProductOptionsResponse();
  tierByName(malformedBullets, "basic").presentation.bullets = [123];

  await assertRejectsProductOptionsBody(
    malformedBullets,
    /Production product-options response contains an unusable tier/,
  );

  const malformedFeatured = minimalProductOptionsResponse();
  tierByName(malformedFeatured, "basic").presentation.is_featured = "yes";

  await assertRejectsProductOptionsBody(
    malformedFeatured,
    /Production product-options response contains an unusable tier/,
  );

  const malformedRibbon = minimalProductOptionsResponse();
  tierByName(malformedRibbon, "basic").presentation.ribbon_label = 42;

  await assertRejectsProductOptionsBody(
    malformedRibbon,
    /Production product-options response contains an unusable tier/,
  );
});

test("fetchProductionProductOptions rejects featured tier without non-empty ribbon label", async () => {
  const body = minimalProductOptionsResponse();
  const basicTier = tierByName(body, "basic");
  basicTier.presentation.is_featured = true;
  basicTier.presentation.ribbon_label = "   ";

  await assertRejectsProductOptionsBody(
    body,
    /Production product-options response contains an unusable tier/,
  );
});

test("fetchProductionProductOptions rejects nonfeatured tier with non-null ribbon label", async () => {
  const body = minimalProductOptionsResponse();
  const basicTier = tierByName(body, "basic");
  basicTier.presentation.is_featured = false;
  basicTier.presentation.ribbon_label = "Basic";

  await assertRejectsProductOptionsBody(
    body,
    /Production product-options response contains an unusable tier/,
  );
});

test("fetchProductionProductOptions rejects malformed premium purchase options in otherwise usable catalogs", async () => {
  const body = minimalProductOptionsResponse();
  const premiumTier = minimalPremiumTier();
  delete premiumTier.purchase_options[0].price.currency_symbol;
  body.tiers.push(premiumTier);

  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return body;
      },
    })),
    /Production product-options response contains an unusable purchase option/,
  );
});

test("fetchProductionProductOptions rejects control-character option id", async () => {
  const body = minimalProductOptionsResponse();
  const premiumTier = minimalPremiumTier();
  premiumTier.purchase_options[0].id = "premium_12_months_usd\n";
  body.tiers.push(premiumTier);

  await assertRejectsProductOptionsBody(
    body,
    /Production product-options response contains an unusable purchase option/,
  );
});

test("fetchProductionProductOptions rejects duplicate option ids", async () => {
  const body = minimalProductOptionsResponse();
  const premiumTier = minimalPremiumTier();
  premiumTier.purchase_options[0].id = "basic_12_months_usd";
  body.tiers.push(premiumTier);

  await assertRejectsProductOptionsBody(
    body,
    /Production product-options response contains duplicate purchase option ids/,
  );
});

test("fetchProductionProductOptions rejects purchase options on unsupported tiers", async () => {
  const body = minimalProductOptionsResponse();
  tierByName(body, "free").purchase_options.push({
    id: "free_12_months_usd",
    term: { quantity: 12, unit: "month", label: "1 year" },
    price: {
      minor_units: 100,
      currency: "USD",
      currency_symbol: "$",
      display_scale: 2,
    },
  });

  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return body;
      },
    })),
    /Production product-options response contains an unusable purchase option/,
  );
});

test("fetchProductionProductOptions rejects v3 capability tiers without total account limit", async () => {
  const body = minimalProductOptionsResponse();
  const basicTier = tierByName(body, "basic");
  basicTier.capability_schema_version = 3;
  delete basicTier.capabilities.limits.synced_accounts;
  basicTier.capabilities.limits.accounts = {};

  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return body;
      },
    })),
    /Production product-options response contains an unusable tier/,
  );
});

test("fetchProductionProductOptions rejects malformed tier feature flags", async () => {
  const body = minimalProductOptionsResponse();
  tierByName(body, "basic").capabilities.features = {
    transaction_history_sync: "yes",
  };

  await assertRejectsProductOptionsBody(
    body,
    /Production product-options response contains an unusable tier/,
  );
});

test("fetchProductionProductOptions rejects out-of-range catalog integers", async () => {
  const tooManyAccounts = minimalProductOptionsResponse();
  tierByName(tooManyAccounts, "basic").capabilities.limits.synced_accounts = 65_536;

  await assertRejectsProductOptionsBody(
    tooManyAccounts,
    /Production product-options response contains an unusable tier/,
  );

  const tooMuchHistory = minimalProductOptionsResponse();
  tierByName(tooMuchHistory, "basic").capabilities.limits.history
    .max_transactions_per_account = 4_294_967_296;

  await assertRejectsProductOptionsBody(
    tooMuchHistory,
    /Production product-options response contains an unusable tier/,
  );

  const unsafeMinorUnits = minimalProductOptionsResponse();
  tierByName(unsafeMinorUnits, "basic").purchase_options[0].price.minor_units =
    Number.MAX_SAFE_INTEGER + 1;

  await assertRejectsProductOptionsBody(
    unsafeMinorUnits,
    /Production product-options response contains an unusable purchase option/,
  );
});

test("fetchProductionProductOptions rejects basic option without display scale", async () => {
  const body = minimalProductOptionsResponse();
  const basicOption = tierByName(body, "basic").purchase_options[0];
  delete basicOption.price.display_scale;
  basicOption.price.decimal_precision = 2;

  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return body;
      },
    })),
    /Production product-options response contains an unusable purchase option/,
  );
});

test("fetchProductionProductOptions rejects fractional or negative display scale", async () => {
  for (const displayScale of [1.5, -1]) {
    const body = minimalProductOptionsResponse();
    tierByName(body, "basic").purchase_options[0].price.display_scale =
      displayScale;

    await assert.rejects(
      () => fetchProductionProductOptions(async () => ({
        ok: true,
        status: 200,
        async json() {
          return body;
        },
      })),
      /Production product-options response contains an unusable purchase option/,
    );
  }
});

test("fetchProductionProductOptions rejects catalogs without usable Basic activation option", async () => {
  await assert.rejects(
    () => fetchProductionProductOptions(async () => ({
      ok: true,
      status: 200,
      async json() {
        return {
          catalog_schema_version: 4,
          tiers: [
            {
              tier: "premium",
              display_name: "Premium",
              capabilities: {
                limits: {
                  synced_accounts: 20,
                  history: { max_transactions_per_account: 1000 },
                },
              },
              presentation: {
                summary: "Premium screenshot tier",
              },
              purchase_options: [
                {
                  id: "premium_12_months_usd",
                  term: { quantity: 12, unit: "month", label: "1 year" },
                  price: {
                    minor_units: 9000,
                    currency: "USD",
                    currency_symbol: "$",
                    display_scale: 2,
                  },
                },
              ],
            },
          ],
        };
      },
    })),
    /Production product-options response must contain usable basic_12_months_usd option/,
  );
});

test("seedMockCentralProductOptions posts productOptionsResponse to mock Central", async () => {
  const productOptionsResponse = minimalProductOptionsResponse();
  let observedPath = null;
  let observedBody = null;
  const requestContext = {
    async post(path, options) {
      observedPath = path;
      observedBody = options.data;
      return {
        ok() {
          return true;
        },
        status() {
          return 200;
        },
      };
    },
  };

  await seedMockCentralProductOptions(
    requestContext,
    "http://127.0.0.1:8082",
    productOptionsResponse,
  );

  assert.equal(observedPath, "http://127.0.0.1:8082/__mock/scenario");
  assert.deepEqual(observedBody, { productOptionsResponse });
});

test("seedMockCentralProductOptions rejects mock Central errors", async () => {
  const requestContext = {
    async post() {
      return {
        ok() {
          return false;
        },
        status() {
          return 500;
        },
      };
    },
  };

  await assert.rejects(
    () => seedMockCentralProductOptions(
      requestContext,
      "http://127.0.0.1:8082",
      minimalProductOptionsResponse(),
    ),
    /mock-central product-options seed failed: 500/,
  );
});
