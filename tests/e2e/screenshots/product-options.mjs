export const PRODUCTION_PRODUCT_OPTIONS_URL =
  "https://bitgarth.com/api/v1/payments/product-options";

export const PRODUCTION_EXPECTED_SIGNING_KEY_HASH =
  "XAAuZWbX29KVe52GSwou0TPAA8GFUxAKdJSKlfC9pHM";

export const EXPECTED_SIGNING_KEY_HASH_HEADER =
  "X-BitGarth-Expected-Signing-Key-Hash";

const BASIC_ACTIVATION_OPTION_ID = "basic_12_months_usd";
const KNOWN_TIER_FEATURE_FLAGS = [
  "historical_sync",
  "transaction_history_sync",
  "balance_sync",
  "exchange_rates_current",
  "exchange_rates_history",
  "price_overrides",
  "balance_assertions",
  "hledger_export",
  "tax_reports",
];

function isObject(value) {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function isNonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function isNumber(value) {
  return typeof value === "number" && Number.isFinite(value);
}

function isU16(value) {
  return Number.isInteger(value) && value >= 0 && value <= 65_535;
}

function isU32(value) {
  return Number.isInteger(value) && value >= 0 && value <= 4_294_967_295;
}

function isU64(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function isNonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0;
}

function isPositiveInteger(value) {
  return Number.isInteger(value) && value > 0;
}

function hasControlCharacter(value) {
  return [...value].some((char) => {
    const codePoint = char.codePointAt(0);
    return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
  });
}

function isValidProductOptionId(value) {
  return isNonEmptyString(value) && !hasControlCharacter(value);
}

function hasValidTierPresentation(presentation) {
  if (!isObject(presentation) || !isNonEmptyString(presentation.summary)) {
    return false;
  }
  if (
    presentation.bullets !== undefined &&
    !(
      Array.isArray(presentation.bullets) &&
      presentation.bullets.every((bullet) => typeof bullet === "string")
    )
  ) {
    return false;
  }
  if (
    presentation.is_featured !== undefined &&
    typeof presentation.is_featured !== "boolean"
  ) {
    return false;
  }
  if (
    presentation.ribbon_label !== undefined &&
    presentation.ribbon_label !== null &&
    typeof presentation.ribbon_label !== "string"
  ) {
    return false;
  }
  if (
    presentation.is_featured === true &&
    !isNonEmptyString(presentation.ribbon_label)
  ) {
    return false;
  }
  if (
    presentation.is_featured !== true &&
    presentation.ribbon_label !== null &&
    presentation.ribbon_label !== undefined
  ) {
    return false;
  }
  return true;
}

function hasSupportedTierCapabilities(tier) {
  const capabilities = tier.capabilities;
  if (!isObject(capabilities)) {
    return false;
  }
  if (
    capabilities.features !== undefined &&
    !(
      isObject(capabilities.features) &&
      KNOWN_TIER_FEATURE_FLAGS.every(
        (field) =>
          capabilities.features[field] === undefined ||
          typeof capabilities.features[field] === "boolean",
      )
    )
  ) {
    return false;
  }

  const limits = capabilities.limits;
  if (!isObject(limits)) {
    return false;
  }

  const history = limits.history;
  if (
    !isObject(history) ||
    !isU32(history.max_transactions_per_account)
  ) {
    return false;
  }

  if (
    tier.capability_schema_version === undefined ||
    tier.capability_schema_version === 2
  ) {
    return isU16(limits.synced_accounts) && limits.synced_accounts > 0;
  }

  if (tier.capability_schema_version === 3) {
    return isObject(limits.accounts) && isU16(limits.accounts.total) && limits.accounts.total > 0;
  }

  return false;
}

function isRenderableTier(tier) {
  if (!isObject(tier) || !isNonEmptyString(tier.tier)) {
    return false;
  }
  if (!isNonEmptyString(tier.display_name) || !Array.isArray(tier.purchase_options)) {
    return false;
  }
  if (!hasValidTierPresentation(tier.presentation)) {
    return false;
  }
  return hasSupportedTierCapabilities(tier);
}

function isUsableOption(tier, option) {
  if (
    !["basic", "premium"].includes(tier.tier) ||
    !isObject(option) ||
    !isValidProductOptionId(option.id)
  ) {
    return false;
  }

  const term = option.term;
  const price = option.price;
  return (
    isObject(term) &&
    isPositiveInteger(term.quantity) &&
    term.quantity <= 65_535 &&
    isNonEmptyString(term.unit) &&
    isNonEmptyString(term.label) &&
    isObject(price) &&
    isU64(price.minor_units) &&
    price.minor_units > 0 &&
    isNonEmptyString(price.currency) &&
    isNonEmptyString(price.currency_symbol) &&
    isU16(price.display_scale) &&
    price.display_scale <= 255
  );
}

function hasDuplicateOptionIds(tiers) {
  const seen = new Set();
  for (const tier of tiers) {
    for (const option of tier.purchase_options) {
      if (seen.has(option.id)) {
        return true;
      }
      seen.add(option.id);
    }
  }
  return false;
}

function validateProductOptions(body) {
  if (!isObject(body)) {
    throw new Error("Production product-options response must be a JSON object");
  }
  if (!isU16(body.catalog_schema_version) || body.catalog_schema_version < 4) {
    throw new Error(
      "Production product-options response must contain catalog_schema_version >= 4",
    );
  }
  if (!Array.isArray(body.tiers)) {
    throw new Error("Production product-options response must contain a tiers array");
  }
  if (body.tiers.length === 0) {
    throw new Error("Production product-options response must contain at least one tier");
  }
  const hasPurchaseOption = body.tiers.some(
    (tier) => Array.isArray(tier?.purchase_options) && tier.purchase_options.length > 0,
  );
  if (!hasPurchaseOption) {
    throw new Error(
      "Production product-options response must contain at least one purchase option",
    );
  }
  if (!body.tiers.every((tier) => isRenderableTier(tier))) {
    throw new Error("Production product-options response contains an unusable tier");
  }
  if (
    !body.tiers.every((tier) =>
      tier.purchase_options.every((option) => isUsableOption(tier, option)),
    )
  ) {
    throw new Error(
      "Production product-options response contains an unusable purchase option",
    );
  }
  if (hasDuplicateOptionIds(body.tiers)) {
    throw new Error(
      "Production product-options response contains duplicate purchase option ids",
    );
  }
  const usableOptions = body.tiers.flatMap((tier) =>
    tier.purchase_options.map((option) => ({ tier, option })),
  );
  if (usableOptions.length === 0) {
    throw new Error(
      "Production product-options response must contain at least one usable paid option",
    );
  }
  if (
    !usableOptions.some(
      ({ tier, option }) =>
        tier.tier === "basic" && option.id === BASIC_ACTIVATION_OPTION_ID,
    )
  ) {
    throw new Error(
      "Production product-options response must contain usable basic_12_months_usd option",
    );
  }
  return body;
}

export async function fetchProductionProductOptions(fetchImpl = fetch) {
  let response;
  try {
    response = await fetchImpl(PRODUCTION_PRODUCT_OPTIONS_URL, {
      headers: {
        [EXPECTED_SIGNING_KEY_HASH_HEADER]: PRODUCTION_EXPECTED_SIGNING_KEY_HASH,
      },
    });
  } catch (error) {
    throw new Error(
      `Failed to fetch production product-options: ${error.message}`,
      { cause: error },
    );
  }

  if (!response.ok) {
    throw new Error(
      `Production product-options request failed: HTTP ${response.status}`,
    );
  }

  let body;
  try {
    body = await response.json();
  } catch (error) {
    throw new Error(
      `Production product-options response was not valid JSON: ${error.message}`,
      { cause: error },
    );
  }

  return validateProductOptions(body);
}

export async function seedMockCentralProductOptions(
  requestContext,
  mockCentralBaseUrl,
  productOptionsResponse,
) {
  const response = await requestContext.post(`${mockCentralBaseUrl}/__mock/scenario`, {
    data: { productOptionsResponse },
  });
  if (!response.ok()) {
    throw new Error(`mock-central product-options seed failed: ${response.status()}`);
  }
}
