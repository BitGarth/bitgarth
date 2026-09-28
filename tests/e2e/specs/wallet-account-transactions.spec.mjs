import { createServer } from "node:http";
import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  TEST_BTC_ADDRESS,
  TEST_ETH_ADDRESS,
  TEST_ETHERSCAN_API_KEY,
  TEST_UNKNOWN_BTC_ADDRESS,
  addAndSyncLimitedBitcoinAccount,
  assertNoBrowserDiagnostics,
  configureMockServers,
  mempoolFixture,
  registerViaUiAndExpectAuthenticated,
  saveEtherscanApiKey,
  saveEtherscanBaseUrl,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

const MOCK_CENTRAL_URL = "http://127.0.0.1:8082";

test.use({
  mempoolAddressData: {
    [TEST_BTC_ADDRESS]: {
      stats: { ...mempoolFixture.knownAddressStats, tx_count: 30 },
      txs: mempoolFixture.transactions,
    },
  },
});

test("paid limited Bitcoin history masks closing balances on desktop and mobile", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START limited-bitcoin-history");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  const { account_id: accountId, history } =
    await addAndSyncLimitedBitcoinAccount(
      page,
      mockServers,
      "E2E Limited Bitcoin Transactions",
    );

  expect(history.confirmed.rows).toHaveLength(2);
  for (const row of history.confirmed.rows) {
    expect(row.balance_reliability).toEqual({
      kind: "provisional",
      reasons: ["historical_coverage_limited"],
    });
  }
  expect(history.pending.rows).toHaveLength(1);
  expect(history.pending.rows[0].closing_balance).toBeNull();

  await page.goto(`/wallets/account/${accountId}/transactions`);
  await expect(page.locator(".tx-header-opening-balance")).toHaveCount(1);
  await expect(page.locator(".tx-header-closing-balance")).toHaveCount(1);
  const notice = page.getByTestId("transaction-sync-pause-notice");
  await expect(notice).toHaveText(
    "Transaction syncing is paused because this account reached its plan limit. Older or newer transactions may be missing. Balances still refresh about every 15 minutes.",
  );
  await expect(page.getByTestId("transaction-history-coverage-notice")).toHaveCount(0);
  await expect(page.locator(".tx-history-coverage")).toHaveCount(0);
  const pendingSection = page
    .getByRole("heading", { name: "Pending / Unconfirmed", exact: true })
    .locator("xpath=ancestor::section[1]");
  await pendingSection.getByRole("button", { name: "Table view" }).click();
  await expect(
    pendingSection.locator(".transactions-table-view .tx-table-balance").filter({
      hasText: "Not available",
    }),
  ).toHaveCount(1);

  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await expect(
    page
      .getByRole("heading", { name: "Pending / Unconfirmed", exact: true })
      .locator("xpath=ancestor::section[1]")
      .locator(".transactions-list .tx-balance-cell")
      .filter({
        hasText: "Not available",
      }),
  ).toHaveCount(1);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END limited-bitcoin-history");
});

test("free limited Bitcoin history shows the current provider balance", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START free-limited-bitcoin-history");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  const { account_id: accountId, history, orderId } = await addAndSyncLimitedBitcoinAccount(
    page,
    mockServers,
    "E2E Free Limited Bitcoin Transactions",
  );

  const scenarioResponse = await page.request.post(`${MOCK_CENTRAL_URL}/__mock/scenario`, {
    data: {
      orderStatus: "paid",
      paidTokenPayload: {
        premium_access_token: "test-token",
        tier: "free",
      },
    },
  });
  expect(scenarioResponse.ok()).toBeTruthy();
  const pollResponse = await page.request.post("/_app/user/payments/premium/poll", {
    data: { order_id: orderId },
  });
  expect(pollResponse.ok()).toBeTruthy();

  await page.goto(`/wallets/account/${accountId}/transactions`);
  const notice = page.getByTestId("transaction-sync-pause-notice");
  await expect(notice).toHaveText(
    "Transaction syncing is not included for this account under your current allowance. Balances still refresh about every 15 minutes.",
  );
  await expect(page.getByTestId("transaction-history-coverage-notice")).toHaveCount(0);
  await expect(page.locator(".tx-history-coverage")).toHaveCount(0);
  expect(history.current_balance_checked_at).toBeTruthy();
  const expectedCurrentBalanceTimestamp = formatDefaultUtcTimestamp(
    history.current_balance_checked_at,
  );
  await expect(page.locator(".tx-header-current-balance")).toHaveText(
    `Current balance as of ${expectedCurrentBalanceTimestamp}: ₿0.00095`,
  );
  await expect(page.locator(".tx-header-opening-balance")).toHaveCount(0);
  await expect(page.locator(".tx-header-closing-balance")).toHaveCount(0);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END free-limited-bitcoin-history");
});

test("free transaction history awaits its first provider balance", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START first-provider-balance");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  // This account is created through the API so the test can observe its
  // pre-sync state. Keep the live wallet sync bridge out of that fixture setup.
  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await configureMockServers(page.request, mockServers);

  const addResponse = await page.request.post("/_app/user/wallets/bitcoin/add", {
    data: {
      request: {
        address: TEST_UNKNOWN_BTC_ADDRESS,
        network: "mainnet",
        wallet_id: null,
        wallet_label: "E2E Awaiting Bitcoin Balance",
        account_label: "Awaiting Bitcoin Balance",
      },
    },
  });
  expect(addResponse.ok()).toBeTruthy();
  const { account_id: accountId } = await addResponse.json();

  await page.goto(`/wallets/account/${accountId}/transactions`);
  await expect(page.locator(".tx-header-current-balance")).toHaveText(
    "Current balance (Awaiting first sync): Not available",
  );

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END first-provider-balance");
});

function txHash(ordinal) {
  return `0x${ordinal.toString(16).padStart(64, "0")}`;
}

function truncateUiHash(value) {
  if (value.length <= 15) {
    return value;
  }
  return `${value.slice(0, 8)}…${value.slice(-6)}`;
}

function sectionTotals(section) {
  return section.locator(".transactions-table-header .muted");
}

function formatLocalDate(date) {
  const year = String(date.getUTCFullYear());
  const month = String(date.getUTCMonth() + 1).padStart(2, "0");
  const day = String(date.getUTCDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function formatDefaultUtcTimestamp(rfc3339) {
  const timestamp = new Date(rfc3339);
  expect(timestamp.valueOf()).not.toBeNaN();
  const date = timestamp.toLocaleDateString("en-US", {
    timeZone: "UTC",
    month: "short",
    day: "2-digit",
    year: "numeric",
  });
  const time = timestamp.toLocaleTimeString("en-US", {
    timeZone: "UTC",
    hour: "2-digit",
    minute: "2-digit",
    hour12: true,
    timeZoneName: "short",
  });
  return `${date} ${time}`;
}

function currentYearRangeForToday() {
  const today = new Date();
  return {
    start: `${today.getUTCFullYear()}-01-01`,
    end: formatLocalDate(today),
  };
}

async function activatePremiumForTransactionHistory(request) {
  const resetResponse = await request.post(`${MOCK_CENTRAL_URL}/__mock/reset`);
  expect(resetResponse.ok()).toBeTruthy();
  const scenarioResponse = await request.post(`${MOCK_CENTRAL_URL}/__mock/scenario`, {
    data: {
      orderStatus: "paid",
      paidTokenPayload: {
        premium_access_token: "test-token",
      },
    },
  });
  expect(scenarioResponse.ok()).toBeTruthy();

  const startResponse = await request.post("/_app/user/payments/premium/start", {
    data: {
      product_option_id: "premium_12_months_usd",
    },
  });
  expect(startResponse.ok()).toBeTruthy();
  const startPayload = await startResponse.json();
  const orderId = startPayload?.central_order_id;
  expect(orderId).toBeTruthy();

  const pollResponse = await request.post("/_app/user/payments/premium/poll", {
    data: {
      order_id: orderId,
    },
  });
  expect(pollResponse.ok()).toBeTruthy();
}

function buildDenseEtherscanTransactions(count, knownAddress, baseDate = new Date()) {
  const baseBlockNumber = 21_500_100;
  const baseTimestamp = Math.floor(
    Date.UTC(
      baseDate.getUTCFullYear(),
      baseDate.getUTCMonth(),
      baseDate.getUTCDate(),
      12,
      0,
      0,
    ) / 1000,
  );
  const incomingAddress = "0x1111111111111111111111111111111111111111";
  const outgoingAddress = "0x2222222222222222222222222222222222222222";
  const rows = [];

  for (let index = 0; index < count; index += 1) {
    const ordinal = index + 1;
    const incoming = index % 2 === 0;
    rows.push({
      blockNumber: String(baseBlockNumber + index),
      timeStamp: String(baseTimestamp + index * 60),
      hash: txHash(ordinal),
      from: incoming ? incomingAddress : knownAddress,
      to: incoming ? knownAddress : outgoingAddress,
      value: incoming ? "500000000000000000" : "1200000000000000000",
      gasPrice: "12000000000",
      gasUsed: "21000",
      isError: "0",
      txreceipt_status: "1",
      nonce: String(index),
    });
  }

  return rows;
}

async function startDenseEtherscanServer(knownAddress, transactions) {
  const knownAddressLower = knownAddress.toLowerCase();

  const server = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const params = url.searchParams;
    const module = params.get("module");
    const action = params.get("action");

    if (module === "proxy" && action === "eth_blockNumber") {
      const blockNumber = 0x14dc938 + transactions.length;
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          result: `0x${blockNumber.toString(16)}`,
        }),
      );
      return;
    }

    if (module === "account" && action === "txlist") {
      const address = (params.get("address") ?? "").toLowerCase();
      const sort = (params.get("sort") ?? "asc").toLowerCase();
      if (address === knownAddressLower) {
        const result = sort === "desc" ? [...transactions].reverse() : transactions;
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            status: "1",
            message: "OK",
            result,
          }),
        );
        return;
      }

      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          status: "0",
          message: "No transactions found",
          result: [],
        }),
      );
      return;
    }

    if (module === "account" && action === "balance") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          status: "1",
          message: "OK",
          result: "2500000000000000000",
        }),
      );
      return;
    }

    if (module === "account" && action === "txlistinternal") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          status: "0",
          message: "No transactions found",
          result: [],
        }),
      );
      return;
    }

    res.writeHead(404, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "not found" }));
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });

  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("Dense etherscan server failed to bind to an IPv4 socket");
  }

  return {
    baseUrl: `http://127.0.0.1:${address.port}/`,
    close: () =>
      new Promise((resolve, reject) => {
        server.close((error) => {
          if (error) {
            reject(error);
            return;
          }
          resolve();
        });
      }),
  };
}

async function createTransactionsAccount({ page, mockServers, premium = false, priorAccounts = 0 }) {
  const denseTransactions = buildDenseEtherscanTransactions(55, TEST_ETH_ADDRESS);
  const denseEtherscan = await startDenseEtherscanServer(
    TEST_ETH_ADDRESS,
    denseTransactions,
  );
  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  if (premium) {
    await activatePremiumForTransactionHistory(page.request);
  }
  await saveEtherscanBaseUrl(page.request, denseEtherscan.baseUrl);
  await saveEtherscanApiKey(page.request, TEST_ETHERSCAN_API_KEY);
  for (let index = 1; index <= priorAccounts; index++) {
    const response = await page.request.post("/_app/user/wallets/ethereum/add", {
      data: {
        request: {
          address: `0x${index.toString(16).padStart(40, "0")}`,
          network: "mainnet",
          wallet_label: `E2E Earlier Wallet ${index}`,
        },
      },
    });
    expect(response.ok(), await response.text()).toBeTruthy();
  }
  const addAddressResponse = await page.request.post(
    "/_app/user/wallets/ethereum/add",
    {
      data: {
        request: {
          address: TEST_ETH_ADDRESS,
          network: "mainnet",
          wallet_label: "E2E Transactions Wallet",
        },
      },
    },
  );
  expect(addAddressResponse.ok()).toBeTruthy();
  const addAddressPayload = await addAddressResponse.json();
  const accountId = addAddressPayload.account_id;
  expect(accountId).toBeTruthy();
  await expect
    .poll(
      async () => {
        const response = await page.request.get(
          "/_app/user/transactions/sync/accounts",
        );
        if (!response.ok()) {
          return false;
        }
        const snapshots = await response.json();
        const snapshot = snapshots.find(({ account_id }) => account_id === accountId);
        return Boolean(snapshot?.last_completed_at) && snapshot.addresses_in_progress === 0;
      },
      { intervals: [250, 500, 1000, 2000], timeout: 30_000 },
    )
    .toBe(true);
  return { accountId, denseEtherscan, denseTransactions };
}

test("wallet transactions page shows deterministic ordering and paging", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);

  const denseTransactions = buildDenseEtherscanTransactions(55, TEST_ETH_ADDRESS);
  const denseEtherscan = await startDenseEtherscanServer(
    TEST_ETH_ADDRESS,
    denseTransactions,
  );

  try {
    await registerViaUiAndExpectAuthenticated(page);
    await configureMockServers(page.request, mockServers);
    await activatePremiumForTransactionHistory(page.request);
    await saveEtherscanBaseUrl(page.request, denseEtherscan.baseUrl);
    await saveEtherscanApiKey(page.request, TEST_ETHERSCAN_API_KEY);

    const addAddressResponse = await page.request.post("/_app/user/wallets/ethereum/add", {
      data: {
        request: {
          address: TEST_ETH_ADDRESS,
          network: "mainnet",
          wallet_label: "E2E Transactions Wallet",
        },
      },
    });
    expect(addAddressResponse.ok()).toBeTruthy();
    const addAddressPayload = await addAddressResponse.json();
    const accountId = addAddressPayload.account_id;
    expect(accountId).toBeTruthy();

    const syncResponse = await page.request.post("/_app/user/transactions/sync", {
      data: { request: { source: "manual" } },
    });
    expect(syncResponse.ok()).toBeTruthy();

    await expect
      .poll(
        async () => {
          const response = await page.request.get(
            `/_app/user/account/${accountId}/transactions?pending_page=1&confirmed_page=1`,
          );
          if (!response.ok()) {
            return -1;
          }
          const payload = await response.json();
          return payload?.confirmed?.total ?? -1;
        },
        {
          intervals: [250, 500, 1000, 2000],
          timeout: 30_000,
        },
      )
      .toBe(55);

    const syncedAccountResponse = await page.request.get(
      `/_app/user/account/${accountId}/transactions?pending_page=1&confirmed_page=1`,
    );
    expect(syncedAccountResponse.ok()).toBeTruthy();
    const syncedAccountPayload = await syncedAccountResponse.json();
    expect(syncedAccountPayload.etherscan_history_status).toBe("continuous");

    await page.goto("/wallets");
    const navigateLink = page.locator(".account-navigate").first();
    await expect(navigateLink).toBeVisible();

    // Sync status mark opens the sync record with timestamps.
    const syncMark = page.getByTestId("sync-mark").first();
    await syncMark.click();
    const syncRecord = page.getByTestId("sync-record");
    await expect(syncRecord).toBeVisible();
    await expect(syncRecord).toContainText("Last successful sync");
    await expect(syncRecord).toContainText("Latest run");
    await page.keyboard.press("Escape");
    await expect(syncRecord).not.toBeVisible();

    await navigateLink.click();

    await expect
      .poll(() => new URL(page.url()).pathname)
      .toBe(`/wallets/account/${accountId}/transactions`);
    await expect
      .poll(() => new URL(page.url()).searchParams.toString())
      .toBe("");

    // Sync status lives in the header row; the record opens with timestamps.
    const headerRow = page.locator(".tx-header-top-row");
    await expect(headerRow.getByTestId("sync-mark")).toBeVisible();
    await expect(headerRow.getByTestId("account-sync-icon")).toBeVisible();
    await headerRow.getByTestId("sync-mark").click();
    await expect(syncRecord).toBeVisible();
    await expect(syncRecord).toContainText("Last successful sync");
    await expect(syncRecord).toContainText("Latest run");
    await page.keyboard.press("Escape");
    await expect(syncRecord).not.toBeVisible();

    // Pending section is only rendered when there are pending transactions;
    // this test has 0 pending, so the section should not be visible.
    await expect(
      page.getByRole("heading", { name: "Pending / Unconfirmed", exact: true }),
    ).not.toBeVisible();

    const confirmedSection = page
      .getByRole("heading", { name: "History", exact: true })
      .locator("xpath=ancestor::section[1]");

    await expect(confirmedSection).toBeVisible();
    await expect(sectionTotals(confirmedSection)).toHaveText("1 - 50 of 55");

    await expect(
      confirmedSection.getByRole("button", { name: "Copy transaction ID" }).first(),
    ).toBeVisible();

    // Default sort is descending (newest first): denseTransactions[54] is the newest.
    const confirmedCards = confirmedSection.locator(".transactions-list .tx-card");
    await expect(confirmedCards).toHaveCount(50);
    await expect(confirmedCards.nth(0).locator(".tx-external-link")).toHaveText(
      truncateUiHash(denseTransactions[54].hash),
    );
    await expect(confirmedCards.nth(0).locator(".tx-external-link")).toHaveAttribute(
      "href",
      new RegExp(`/tx/${denseTransactions[54].hash}$`),
    );
    await expect(confirmedCards.nth(0).locator(".tx-external-link")).toHaveAttribute(
      "title",
      denseTransactions[54].hash,
    );
    await confirmedCards.nth(0).getByRole("button", { name: "Copy transaction ID" }).click();
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(
      denseTransactions[54].hash,
    );
    await expect(confirmedCards.nth(1).locator(".tx-external-link")).toHaveText(
      truncateUiHash(denseTransactions[53].hash),
    );
    await expect(confirmedCards.nth(1).locator(".tx-external-link")).toHaveAttribute(
      "href",
      new RegExp(`/tx/${denseTransactions[53].hash}$`),
    );

    const confirmedNextButton = confirmedSection.getByRole("button", {
      name: /^Next/,
    }).first();
    await expect(confirmedNextButton).toBeEnabled();
    await confirmedNextButton.click();

    // Page 2 (rows 51-55): oldest 5 transactions in descending order.
    await expect(sectionTotals(confirmedSection)).toHaveText("51 - 55 of 55");
    const secondPageCards = confirmedSection.locator(".transactions-list .tx-card");
    await expect(secondPageCards).toHaveCount(5);
    await expect(secondPageCards.nth(0).locator(".tx-external-link")).toHaveText(
      truncateUiHash(denseTransactions[4].hash),
    );
    await expect(secondPageCards.nth(0).locator(".tx-external-link")).toHaveAttribute(
      "href",
      new RegExp(`/tx/${denseTransactions[4].hash}$`),
    );

    const confirmedPreviousButton = confirmedSection.getByRole("button", {
      name: /^Previous/,
    }).first();
    await expect(confirmedPreviousButton).toBeEnabled();
    await confirmedPreviousButton.click();

    await expect(sectionTotals(confirmedSection)).toHaveText("1 - 50 of 55");
    await expect(
      confirmedSection
        .locator(".transactions-list .tx-card")
        .nth(0)
        .locator(".tx-external-link"),
    ).toHaveText(truncateUiHash(denseTransactions[54].hash));

    assertNoBrowserDiagnostics(diagnostics);
    await markTestBoundary(testInfo, "END");
  } finally {
    await denseEtherscan.close();
  }
});

test("failed Ethereum attempts stay in History with fee-only balances", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START failed-ethereum-history");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  const year = new Date().getUTCFullYear();
  const timestamp = (day) => String(Date.UTC(year, 0, day, 12) / 1000);
  const transactions = [
    {
      blockNumber: "21500100", timeStamp: timestamp(1), hash: txHash(1001),
      from: "0x1111111111111111111111111111111111111111", to: TEST_ETH_ADDRESS,
      value: "2000000000000000000", gasPrice: "1000000000", gasUsed: "21000",
      isError: "0", txreceipt_status: "1", nonce: "1",
    },
    {
      blockNumber: "21500101", timeStamp: timestamp(2), hash: txHash(1002),
      from: TEST_ETH_ADDRESS, to: "0x2222222222222222222222222222222222222222",
      value: "400000000000000000", gasPrice: "1000000000", gasUsed: "1000000",
      isError: "1", txreceipt_status: "0", nonce: "2",
    },
    {
      blockNumber: "21500102", timeStamp: timestamp(3), hash: txHash(1003),
      from: "0x3333333333333333333333333333333333333333", to: TEST_ETH_ADDRESS,
      value: "300000000000000000", gasPrice: "1000000000", gasUsed: "21000",
      isError: "1", txreceipt_status: "0", nonce: "3",
    },
  ];
  const etherscan = await startDenseEtherscanServer(TEST_ETH_ADDRESS, transactions);

  try {
    await registerViaUiAndExpectAuthenticated(page);
    await configureMockServers(page.request, mockServers);
    await activatePremiumForTransactionHistory(page.request);
    await saveEtherscanBaseUrl(page.request, etherscan.baseUrl);
    await saveEtherscanApiKey(page.request, TEST_ETHERSCAN_API_KEY);
    const addResponse = await page.request.post("/_app/user/wallets/ethereum/add", {
      data: { request: {
        address: TEST_ETH_ADDRESS, network: "mainnet",
        wallet_label: "E2E Failed Ethereum History",
      } },
    });
    expect(addResponse.ok()).toBeTruthy();
    const { account_id: accountId } = await addResponse.json();
    const syncResponse = await page.request.post("/_app/user/transactions/sync", {
      data: { request: { source: "manual" } },
    });
    expect(syncResponse.ok()).toBeTruthy();
    await expect.poll(async () => {
      const response = await page.request.get(
        `/_app/user/account/${accountId}/transactions?pending_page=1&confirmed_page=1`,
      );
      return response.ok() ? (await response.json()).confirmed?.total : -1;
    }, { intervals: [250, 500, 1000, 2000], timeout: 30_000 }).toBe(3);

    await page.goto(`/wallets/account/${accountId}/transactions`);
    const history = page.getByRole("heading", { name: "History", exact: true })
      .locator("xpath=ancestor::section[1]");
    await expect(history).toBeVisible();
    await expect(history.locator(".tx-card")).toHaveCount(3);
    await page.getByRole("button", { name: /^failed$/i }).click();
    await expect(history.locator(".tx-card")).toHaveCount(2);
    await expect(history.getByText("Not transferred", { exact: true }).first()).toBeVisible();
    await history.getByRole("button", { name: "Table view" }).click();
    const rows = history.locator("tbody tr");
    await expect(rows).toHaveCount(2);
    await expect(rows.first().locator(".tx-table-hash a")).toHaveText(
      truncateUiHash(transactions[2].hash),
    );
    await expect(rows.first().locator(".tx-table-hash a")).toHaveAttribute(
      "href",
      new RegExp(`/tx/${transactions[2].hash}$`),
    );
    await expect(rows.first().locator(".tx-table-amount s")).toHaveText("Ξ0.3");
    await expect(rows.nth(1).locator(".tx-table-amount s")).toHaveText("-Ξ0.4");
    await expect(rows.first().locator(".tx-table-amount")).toContainText("Not transferred");
    await expect(rows.nth(1).locator(".tx-table-amount")).toContainText("Not transferred");
    await expect(rows.first()).toContainText(formatDefaultUtcTimestamp(new Date(Number(transactions[2].timeStamp) * 1000).toISOString()));
    await expect(rows.nth(1).locator(".tx-table-fee")).toHaveText("-Ξ0.001");
    await expect(rows.first().locator(".tx-table-balance")).toContainText("1.999");
    await expect(rows.nth(1).locator(".tx-table-balance")).toContainText("1.999");
    await expect(rows.first().locator(".tx-table-fee")).toHaveText("-");
    await testInfo.attach("failed-history-wide", {
      body: await history.screenshot(), contentType: "image/png",
    });

    await expect(sectionTotals(history)).toHaveCount(0);

    await page.setViewportSize({ width: 390, height: 844 });
    await history.getByRole("button", { name: "Card view" }).click();
    const cards = history.locator(".tx-card");
    await expect(cards.first().locator(".tx-card-amount s")).toHaveText("Ξ0.3");
    await expect(cards.nth(1).locator(".tx-card-amount s")).toHaveText("-Ξ0.4");
    await expect(cards.first().getByText("Not transferred")).toBeVisible();
    await expect(cards.nth(1).getByText("Not transferred")).toBeVisible();
    await expect(cards.nth(1).locator(".tx-card-timestamp")).toHaveText(
      formatDefaultUtcTimestamp(new Date(Number(transactions[1].timeStamp) * 1000).toISOString()),
    );
    await expect(cards.nth(1).locator(".tx-fee-value")).toHaveText("-Ξ0.001");
    await expect(cards.nth(1).locator(".tx-balance-cell")).toHaveText("Ξ1.999");
    await expect(cards.first().locator(".tx-fee-value")).toHaveText("-");
    await expect(cards.first().locator(".tx-balance-cell")).toHaveText("Ξ1.999");
    await expect(cards.first().locator(".tx-status-indicator")).toHaveText("failed");
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    await testInfo.attach("failed-history-390px", {
      body: await history.screenshot(), contentType: "image/png",
    });

    assertNoBrowserDiagnostics(diagnostics);
    await markTestBoundary(testInfo, "END failed-ethereum-history");
  } finally {
    await etherscan.close();
  }
});

test("wallet transactions date toolbar keeps route-backed presets and status filters working", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  // All activity is in the previous year, so the current year is empty (EX-05).
  const previousYear = new Date().getUTCFullYear() - 1;
  const denseTransactions = buildDenseEtherscanTransactions(
    55,
    TEST_ETH_ADDRESS,
    new Date(Date.UTC(previousYear, 5, 15)),
  );
  const denseEtherscan = await startDenseEtherscanServer(
    TEST_ETH_ADDRESS,
    denseTransactions,
  );

  try {
    const expectedThisYear = currentYearRangeForToday();

    await registerViaUiAndExpectAuthenticated(page);
    await configureMockServers(page.request, mockServers);
    await activatePremiumForTransactionHistory(page.request);
    await saveEtherscanBaseUrl(page.request, denseEtherscan.baseUrl);
    await saveEtherscanApiKey(page.request, TEST_ETHERSCAN_API_KEY);

    const addAddressResponse = await page.request.post("/_app/user/wallets/ethereum/add", {
      data: {
        request: {
          address: TEST_ETH_ADDRESS,
          network: "mainnet",
          wallet_label: "E2E Transactions Wallet",
        },
      },
    });
    expect(addAddressResponse.ok()).toBeTruthy();
    const addAddressPayload = await addAddressResponse.json();
    const accountId = addAddressPayload.account_id;
    expect(accountId).toBeTruthy();

    const syncResponse = await page.request.post("/_app/user/transactions/sync", {
      data: { request: { source: "manual" } },
    });
    expect(syncResponse.ok()).toBeTruthy();

    await expect
      .poll(
        async () => {
          const response = await page.request.get(
            `/_app/user/account/${accountId}/transactions?pending_page=1&confirmed_page=1`,
          );
          if (!response.ok()) {
            return -1;
          }
          const payload = await response.json();
          return payload?.confirmed?.total ?? -1;
        },
        {
          intervals: [250, 500, 1000, 2000],
          timeout: 30_000,
        },
      )
      .toBe(55);

    await page.goto(`/wallets/account/${accountId}/transactions`);
    await expect
      .poll(() => new URL(page.url()).pathname)
      .toBe(`/wallets/account/${accountId}/transactions`);
    await expect
      .poll(() => new URL(page.url()).searchParams.toString())
      .toBe("");

    // Defaults to all time: last year's transactions show, paginated, and the
    // custom date inputs stay collapsed.
    const yearValue = page.locator(".date-range-year-value");
    const allTimeButton = page.getByRole("button", { name: "All time", exact: true });
    await expect(yearValue).toHaveText("All time");
    await expect(allTimeButton).toHaveCount(0);
    await expect(page.locator("#account-transactions-start")).toHaveCount(0);

    const confirmedSection = page
      .getByRole("heading", { name: "History", exact: true })
      .locator("xpath=ancestor::section[1]");

    await expect(confirmedSection).toBeVisible();
    await expect(sectionTotals(confirmedSection)).toHaveText("1 - 50 of 55");

    // This Year narrows to the (empty) current year; stepping back finds them.
    await page.getByRole("button", { name: "This Year", exact: true }).click();
    await expect(yearValue).toHaveText(expectedThisYear.start.slice(0, 4));
    await expect
      .poll(() => new URL(page.url()).searchParams.get("start"))
      .toBe(expectedThisYear.start);
    await expect(confirmedSection.getByText("No transactions found")).toBeVisible();

    await page.getByRole("button", { name: "Previous year" }).click();
    await expect(yearValue).toHaveText(String(previousYear));
    await expect(sectionTotals(confirmedSection)).toHaveText("1 - 50 of 55");

    // All time returns to the bare route.
    await allTimeButton.click();
    await expect(yearValue).toHaveText("All time");
    await expect
      .poll(() => new URL(page.url()).searchParams.toString())
      .toBe("");
    await expect(allTimeButton).toHaveCount(0);
    await expect(sectionTotals(confirmedSection)).toHaveText("1 - 50 of 55");

    const confirmedNextButton = confirmedSection.getByRole("button", {
      name: /^Next/,
    }).first();
    await expect(confirmedNextButton).toBeEnabled();
    await confirmedNextButton.click();
    await expect(sectionTotals(confirmedSection)).toHaveText("51 - 55 of 55");

    // Status filter: narrowing to Failed hides the (confirmed) dense rows; the
    // explicit "All" chip restores them.
    await page.getByRole("button", { name: /^failed$/i }).click();
    await expect(confirmedSection.getByText("No transactions found")).toBeVisible();

    await page.getByRole("button", { name: "All", exact: true }).click();
    await expect(sectionTotals(confirmedSection)).toHaveText("1 - 50 of 55");

    assertNoBrowserDiagnostics(diagnostics);
    await markTestBoundary(testInfo, "END");
  } finally {
    await denseEtherscan.close();
  }
});

test("Free first Ethereum account syncs both history and current balance", async ({
  page,
  mockServers,
}) => {
  const { accountId, denseEtherscan } = await createTransactionsAccount({
    page,
    mockServers,
  });
  try {
    await page.goto(`/wallets/account/${accountId}/transactions`);
    await expect(page.getByTestId("account-mode")).toHaveText("Transaction syncing");
    await expect(page.locator(".transactions-list .tx-card").first()).toBeVisible();
    await expect(page.locator(".tx-header-current-balance")).toContainText(
      "Current balance as of",
    );
  } finally {
    await denseEtherscan.close();
  }
});

test("upgrade moves a fourth Ethereum account from balances to transactions", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START upgrade-history-transition");
  const { accountId, denseEtherscan } = await createTransactionsAccount({
    page,
    mockServers,
    priorAccounts: 3,
  });
  try {
    await page.goto(`/wallets/account/${accountId}/transactions`);
    await expect(page.getByTestId("account-mode")).toHaveText("Balance only");
    await expect(page.locator(".tx-header-current-balance")).toContainText(
      "Current balance as of",
      { timeout: 30_000 },
    );
    await expect(page.getByTestId("transaction-sync-pause-notice")).toContainText(
      "current allowance",
    );

    await activatePremiumForTransactionHistory(page.request);
    await page.reload();
    await expect(page.getByTestId("account-mode")).toHaveText("Transaction syncing");
    await expect(page.locator(".transactions-list .tx-card").first()).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.locator(".tx-header-opening-balance")).not.toContainText(
      "Not available",
    );
  } finally {
    await denseEtherscan.close();
  }
});

test("manual sync click reports an outcome", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START manual-sync-outcome");
  const { accountId, denseEtherscan } = await createTransactionsAccount({
    page,
    mockServers,
    premium: true,
  });
  try {
    await page.goto(`/wallets/account/${accountId}/transactions`);
    await expect(page.locator(".transactions-list .tx-card").first()).toBeVisible({
      timeout: 30_000,
    });
    await page.getByTestId("account-sync-icon").click();
    await expect(page.getByTestId("manual-sync-outcome")).toBeVisible({
      timeout: 30_000,
    });
    await expect(page.getByTestId("manual-sync-outcome")).toContainText(
      /Already up to date|Synced/,
    );
  } finally {
    await denseEtherscan.close();
  }
});

test("account identity section shows type, reference, and addresses", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START identity-section");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  const { denseEtherscan } = await createTransactionsAccount({ page, mockServers });

  try {
    await page.goto("/wallets");
    const navigateLink = page.locator(".account-navigate").first();
    await expect(navigateLink).toBeVisible();
    await navigateLink.click();

    const strip = page.getByTestId("account-identity-strip");
    await expect(strip).toBeVisible();
    await expect(strip).toContainText("§ Account");
    await expect(strip).toContainText("Ethereum");
    await expect(strip).toContainText("…");
    await expect(strip).toHaveAttribute("aria-expanded", "false");
    await expect(page.getByTestId("account-identity-panel")).toHaveCount(0);

    await strip.click();
    await expect(strip).toHaveAttribute("aria-expanded", "true");
    const panel = page.getByTestId("account-identity-panel");
    await expect(panel).toBeVisible();
    await expect(page.getByTestId("account-identity-type")).toHaveText("Ethereum address");
    await expect(page.getByTestId("account-identity-reference")).toHaveText(
      TEST_ETH_ADDRESS,
      { ignoreCase: true },
    );
    await expect(panel.getByRole("button", { name: "Copy Address" })).toBeVisible();

    await page.getByTestId("account-identity-view-addresses").click();
    await expect(
      page.getByRole("heading", { name: /^Account addresses/ }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Close" }).click();

    await page.getByRole("button", { name: "Account actions" }).click();
    const menu = page.locator(".kebab-menu-dropdown.visible");
    await expect(menu.getByRole("menuitem", { name: "Rename" })).toBeVisible();
    await expect(menu.getByRole("menuitem", { name: "View Addresses" })).toHaveCount(0);
    await expect(menu.getByRole("menuitem", { name: "Copy Address" })).toHaveCount(0);
  } finally {
    await denseEtherscan.close();
  }

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END identity-section");
});

test("manual asset identity section shows unit and precision", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START manual-identity");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);

  const addManualAssetResponse = await page.request.post(
    "/_app/user/wallets/manual-assets/add",
    {
      data: {
        request: {
          wallet_label: "E2E Manual Identity",
          asset_instance_id: {
            asset_id: "monero",
            network_id: "monero-mainnet",
          },
        },
      },
    },
  );
  expect(addManualAssetResponse.ok()).toBeTruthy();
  const addManualAssetPayload = await addManualAssetResponse.json();
  const accountId = addManualAssetPayload.account_id;
  expect(accountId).toBeTruthy();

  // AddManualAssetAccountResponse.account_id is a WalletAccountId — the
  // exact type the AccountTransactions route takes, so navigate directly.
  await page.goto(`/wallets/account/${accountId}/transactions`);

  const strip = page.getByTestId("account-identity-strip");
  await expect(strip).toBeVisible();
  await expect(strip).toContainText("Manual asset");

  await strip.click();
  const panel = page.getByTestId("account-identity-panel");
  await expect(panel).toBeVisible();
  await expect(page.getByTestId("account-identity-type")).toHaveText("Manual asset");
  await expect(panel).toContainText("Unit");
  await expect(panel).toContainText("Precision");
  await expect(page.getByText("Manual asset (legacy)")).toHaveCount(0);
  await expect(page.getByRole("button", { name: /migrate/i })).toHaveCount(0);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END manual-identity");
});

test("balance assertion dialog keeps keyboard focus and draft", async ({ page, mockServers }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  const accountResponse = await page.request.post("/_app/user/wallets/manual-assets/add", {
    data: {
      request: {
        wallet_label: "Modal focus wallet",
        asset_instance_id: { asset_id: "monero", network_id: "monero-mainnet" },
      },
    },
  });
  expect(accountResponse.ok()).toBeTruthy();
  const { account_id: accountId } = await accountResponse.json();
  const assertionResponse = await page.request.post("/_app/user/manual-asset-assertions/add", {
    data: {
      request: { account_id: accountId, asserted_on: "2025-01-01", balance: "1", note: null },
    },
  });
  expect(assertionResponse.ok()).toBeTruthy();
  await page.goto(`/wallets/account/${accountId}/transactions`);

  const edit = page.getByRole("button", { name: "Edit", exact: true });
  await edit.click();
  const dialog = page.getByRole("dialog", { name: "Edit Balance Assertion" });
  await expect(dialog).toBeVisible();
  const balance = dialog.locator("#custom-assertion-balance");
  await balance.fill("123");
  for (let index = 0; index < 8; index += 1) {
    await page.keyboard.press("Tab");
    const outsideActionFocused = await dialog.evaluate((element) =>
      document.activeElement !== document.body && !element.contains(document.activeElement),
    );
    expect(outsideActionFocused).toBe(false);
  }
  await expect(balance).toHaveValue("123");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
});

test("balance assertions header fits phone width", async ({ page, mockServers }, testInfo) => {
  await markTestBoundary(testInfo, "START assertions-header-phone");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);

  const addManualAssetResponse = await page.request.post("/_app/user/wallets/manual-assets/add", {
    data: {
      request: {
        wallet_label: "E2E Assertions Header",
        asset_instance_id: { asset_id: "monero", network_id: "monero-mainnet" },
      },
    },
  });
  expect(addManualAssetResponse.ok()).toBeTruthy();
  const { account_id: accountId } = await addManualAssetResponse.json();

  // One more than a page, so the header shows its "x - y of n" totals.
  for (let day = 0; day < 51; day += 1) {
    const assertedOn = new Date(Date.UTC(2025, 0, 1 + day)).toISOString().slice(0, 10);
    const response = await page.request.post("/_app/user/manual-asset-assertions/add", {
      data: {
        request: { account_id: accountId, asserted_on: assertedOn, balance: "1", note: null },
      },
    });
    expect(response.ok()).toBeTruthy();
  }

  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`/wallets/account/${accountId}/transactions`);
  const section = page
    .getByRole("heading", { name: "Balance Assertions", exact: true })
    .locator("xpath=ancestor::section[1]");
  const totals = sectionTotals(section);
  await expect(totals).toHaveText("1 - 50 of 51");

  // Squeezed flex items wrap word by word; a wrapping header keeps each on one line.
  for (const item of [totals, section.getByRole("button", { name: "Add Balance Assertion" })]) {
    const fontSize = await item.evaluate((el) => parseFloat(getComputedStyle(el).fontSize));
    expect((await item.boundingBox()).height).toBeLessThan(fontSize * 3);
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await testInfo.attach("assertions-header-390px", {
    body: await section.locator(".transactions-table-header").screenshot(), contentType: "image/png",
  });

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END assertions-header-phone");
});
