import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  configureMockServers,
  registerViaUiAndExpectAuthenticated,
  TEST_XPUB,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

// Native SegWit receive addresses of TEST_XPUB (m/0/10 and m/0/25). Index 10 is
// in the initially derived gap window; index 25 is only reached by discovery.
const XPUB_RECEIVE_10 = "bc1qcq0kekes8u8ajx8wezn7fcnt0zu40ak3a44uvl";
const XPUB_RECEIVE_25 = "bc1qrsukwv3pm8lf2au4xm5j04l5hc3tvfhssutptc";
const SENDER = "bc1q40s4njz45yfr3q0maqh2pjzk5fxfnpmzqqrz09";
// Recent, so the transactions page's default current-year view shows them.
const BLOCK_TIME = Math.floor(Date.now() / 1000) - 3600;

function incomingTx(txid, address, value, blockHeight) {
  return {
    txid,
    vin: [
      {
        txid: "f".repeat(64),
        vout: 0,
        prevout: { scriptpubkey_address: SENDER, value: value + 1000 },
      },
    ],
    vout: [
      {
        scriptpubkey: "0014abcdef1234567890abcdef1234567890abcdef12",
        scriptpubkey_address: address,
        value,
      },
    ],
    fee: 1000,
    status: {
      confirmed: true,
      block_height: blockHeight,
      block_hash: "0".repeat(64),
      block_time: BLOCK_TIME,
    },
  };
}

function fundedAddress(tx, value) {
  return {
    stats: { funded_txo_sum: value, spent_txo_sum: 0, tx_count: 1 },
    txs: [tx],
  };
}

test.use({
  mempoolAddressData: {
    "*": { stats: { funded_txo_sum: 0, spent_txo_sum: 0, tx_count: 0 }, txs: [] },
    [XPUB_RECEIVE_10]: fundedAddress(
      incomingTx("1".repeat(64), XPUB_RECEIVE_10, 50_000, 899_990),
      50_000,
    ),
    [XPUB_RECEIVE_25]: fundedAddress(
      incomingTx("2".repeat(64), XPUB_RECEIVE_25, 70_000, 899_995),
      70_000,
    ),
  },
});

test("added Bitcoin xpub account syncs its transactions without a manual sync", async ({
  page,
  mockServers,
}, testInfo) => {
  // Discovery is stats-only; history arrives on the automatic follow-up run.
  test.setTimeout(240_000);
  await markTestBoundary(testInfo, "START xpub-auto-sync");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);

  const addResponse = await page.request.post("/_app/user/wallets/xpub/add", {
    data: {
      request: {
        extended_pubkey: TEST_XPUB,
        address_scheme: "native_segwit",
        wallet_label: "E2E Auto Sync Wallet",
      },
    },
  });
  expect(addResponse.ok(), await addResponse.text()).toBeTruthy();
  const { account_id: accountId } = await addResponse.json();
  expect(accountId).toBeTruthy();

  // No manual sync: only the automatic add sync and scheduler may fetch.
  await expect
    .poll(
      async () => {
        const response = await page.request.get(
          `/_app/user/account/${accountId}/transactions?pending_page=1&confirmed_page=1`,
        );
        if (!response.ok()) {
          return -1;
        }
        return (await response.json())?.confirmed?.total ?? -1;
      },
      { timeout: 180_000, intervals: [2_000] },
    )
    .toBe(2);

  await page.goto(`/wallets/account/${accountId}/transactions`);
  await expect(page.getByTestId("account-mode")).toHaveText("Transaction syncing");
  await expect(page.locator(".transactions-list .tx-card")).toHaveCount(2);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END xpub-auto-sync");
});
