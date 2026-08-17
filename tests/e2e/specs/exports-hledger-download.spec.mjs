import { execFileSync } from "node:child_process";
import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  addAndSyncLimitedBitcoinAccount,
  assertNoBrowserDiagnostics,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

const HLEDGER_FILENAME_PATTERN = /^bitgarth-hledger-[a-zA-Z0-9._-]+-\d{8}\.zip$/;

test("incomplete Bitcoin hledger export keeps postings and omits closing assertions", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START incomplete-bitcoin-hledger");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  await registerViaUiAndExpectAuthenticated(page);
  await addAndSyncLimitedBitcoinAccount(
    page,
    mockServers,
    "E2E Incomplete Bitcoin Export",
  );

  await page.goto("/exports/hledger");
  await page.getByTestId("hledger-export-encrypted-checkbox").uncheck();
  const downloadPromise = page.waitForEvent("download");
  await page.getByTestId("hledger-export-button").click();
  const download = await downloadPromise;
  const downloadPath = await download.path();
  expect(downloadPath).toBeTruthy();
  const journal = execFileSync("unzip", ["-p", downloadPath], {
    encoding: "utf8",
  });

  expect(journal).toContain(
    "; Transaction a1b2c3d4e5f6071829304050607080901a2b3c4d5e6f071829304050607080ab",
  );
  expect(journal).toContain(" BTC");
  expect(journal).not.toContain("equity:Closing Balances");

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END incomplete-bitcoin-hledger");
});

test("hledger export downloads an unencrypted zip with the expected filename", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/exports/hledger");

  const encryptedCheckbox = page.getByTestId("hledger-export-encrypted-checkbox");
  await expect(encryptedCheckbox).toBeChecked();
  await encryptedCheckbox.uncheck();
  await expect(
    page.getByTestId("hledger-export-unencrypted-warning"),
  ).toContainText("This ZIP is not encrypted");

  const downloadPromise = page.waitForEvent("download");
  await page.getByTestId("hledger-export-button").click();
  const download = await downloadPromise;

  expect(download.suggestedFilename()).toMatch(HLEDGER_FILENAME_PATTERN);
  await expect(page.getByText("Downloaded")).toBeVisible({ timeout: 10_000 });

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("hledger export with encryption enabled downloads after password is confirmed", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await page.goto("/exports/hledger");

  await expect(page.getByTestId("hledger-export-encrypted-checkbox")).toBeChecked();
  const button = page.getByTestId("hledger-export-button");
  await expect(button).toBeDisabled();

  const passphrase = "the-correct-horse-battery-staple-passphrase";
  await page.getByTestId("hledger-export-password").fill(passphrase);
  await expect(button).toBeDisabled();
  await page.getByTestId("hledger-export-confirm-password").fill(passphrase);
  await expect(button).toBeEnabled();

  const downloadPromise = page.waitForEvent("download");
  await button.click();
  const download = await downloadPromise;

  expect(download.suggestedFilename()).toMatch(HLEDGER_FILENAME_PATTERN);
  await expect(page.getByText(/Downloaded .*\(encrypted\)/)).toBeVisible({
    timeout: 10_000,
  });

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
