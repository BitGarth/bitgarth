import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  configureMockServers,
  DEFAULT_TEST_PASSWORD,
  loginViaUi,
  logoutViaUserMenu,
  registerViaUiAndExpectAuthenticated,
  waitForGeneratedUsername,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

test("register and login via browser while capturing diagnostics", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  const credentials = await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  await logoutViaUserMenu(page);
  await loginViaUi(page, credentials.username, credentials.password);
  assertNoBrowserDiagnostics(diagnostics);

  await markTestBoundary(testInfo, "END");
});

test("registration requires terms and privacy acknowledgement", async ({ page }) => {
  await page.goto("/register");
  await page.locator("#password").fill(DEFAULT_TEST_PASSWORD);
  await page.locator("#confirm-password").fill(DEFAULT_TEST_PASSWORD);
  await waitForGeneratedUsername(page);

  const submit = page.locator("form button[type='submit']");
  await expect(submit).toBeDisabled();

  await page.getByTestId("legal-acknowledgement-checkbox").check();
  await expect(submit).toBeEnabled();
});
