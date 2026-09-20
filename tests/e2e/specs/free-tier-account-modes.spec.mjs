import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  TEST_BTC_ADDRESS,
  addAndSyncLimitedBitcoinAccount,
  configureMockServers,
  mempoolFixture,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";

function accountFixture(index) {
  return {
    label: `ETH ${String(index).padStart(3, "0")}`,
    asset_id: "ethereum",
    network: "mainnet",
    account_kind: "single_address",
    created_at: new Date(Date.UTC(2026, 3, 4, 12, index)).toISOString(),
    sync_slot: null,
    hd_keys: [],
    addresses: [{
      address: `0x${index.toString(16).padStart(40, "0")}`,
      address_scheme: "standard",
      source_type: "imported",
    }],
  };
}

async function importFiftyOneAccounts(request) {
  const payload = {
    version: 6,
    exported_at: "2026-04-04T12:00:00Z",
    bitgarth_version: "0.1.0",
    wallets: [{
      label: "Free allowance fixture",
      master_fingerprint: null,
      identity_source: "user_provided",
      verified_at: null,
      accessors: [],
      digital_asset_accounts: Array.from({ length: 51 }, (_, index) => accountFixture(index + 1)),
      manual_asset_accounts: [],
    }],
    settings: null,
  };
  const response = await request.post("/_app/user/imports/wallet-data", {
    data: {
      request: {
        file_name: "wallet-data.json",
        payload_base64: Buffer.from(JSON.stringify(payload)).toString("base64"),
        password: null,
      },
    },
  });
  expect(response.ok(), await response.text()).toBeTruthy();
  const result = await response.json();
  expect(result.native_accounts_created).toHaveLength(51);
}

test("Free account modes follow the first three and first fifty admissions", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await importFiftyOneAccounts(page.request);

  const response = await page.request.get("/_app/user/wallets");
  expect(response.ok()).toBeTruthy();
  const { wallets } = await response.json();
  const accounts = wallets.find((wallet) => wallet.label === "Free allowance fixture")
    ?.accounts.filter((account) => account.kind === "native");
  expect(accounts).toHaveLength(51);
  const byLabel = new Map(accounts.map((account) => [account.label, account]));
  for (const index of [1, 2, 3]) {
    const account = byLabel.get(`ETH ${String(index).padStart(3, "0")}`);
    expect(account.account_mode).toBe("transactions");
  }
  expect(byLabel.get("ETH 004").account_mode).toBe("balance_only");
  expect(byLabel.get("ETH 050").account_mode).toBe("balance_only");
  expect(byLabel.get("ETH 051").account_mode).toBe("inactive");

  for (const [label, reason] of [["ETH 004", "account_allowance"], ["ETH 051", "inactive"]]) {
    const account = byLabel.get(label);
    const history = await page.request.get(`/_app/user/account/${account.account_id}/transactions?pending_page=1&confirmed_page=1`);
    expect(history.ok()).toBeTruthy();
    expect((await history.json()).transaction_sync_pause_reason).toBe(reason);
  }

  await page.goto("/wallets");
  for (const [label, mode] of [["ETH 001", "Transaction syncing"], ["ETH 004", "Balance only"], ["ETH 051", "Inactive"]]) {
    await expect(page.locator(".account-row").filter({ hasText: label }).getByTestId("account-mode"))
      .toHaveText(mode);
  }
  await expect(page.getByRole("button", { name: /select for sync|replace sync/i })).toHaveCount(0);
});

test.use({
  mempoolAddressData: {
    [TEST_BTC_ADDRESS]: {
      stats: { ...mempoolFixture.knownAddressStats, tx_count: 30 },
      txs: mempoolFixture.transactions,
    },
  },
});

test("threshold history notice survives a balance refresh", async ({ page, mockServers }) => {
  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  const { account_id: accountId } = await addAndSyncLimitedBitcoinAccount(
    page, mockServers, "Threshold history fixture",
  );

  const accountUrl = `/wallets/account/${accountId}/transactions`;
  await page.goto(accountUrl);
  const notice = page.getByTestId("transaction-sync-pause-notice");
  await expect(notice).toHaveText(
    "Transaction syncing is paused because this account reached its plan limit. Older or newer transactions may be missing. Balances continue to update independently.",
  );
  await expect(page.getByTestId("transaction-history-coverage-notice")).toHaveCount(0);

  const refresh = await page.request.post("/_app/user/transactions/sync", {
    data: { request: { source: "manual", scope: { kind: "account", account_id: accountId } } },
  });
  expect(refresh.ok(), await refresh.text()).toBeTruthy();
  await page.reload();
  await expect(notice).toBeVisible();
  await expect(page.getByTestId("account-mode")).toHaveText("Transaction syncing");
});
