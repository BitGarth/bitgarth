import { expect, test } from "../helpers/mock-fixture.mjs";
import { assertNoBrowserDiagnostics } from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

test("wallets route redirects logged-out users to login", async ({ page }, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await page.goto("/wallets");
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.locator("#username")).toBeVisible();

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("settings route redirects logged-out users to login", async ({ page }, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await page.goto("/settings");
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.locator("#username")).toBeVisible();

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
