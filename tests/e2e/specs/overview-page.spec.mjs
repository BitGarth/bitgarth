import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  configureMockServers,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

test("authenticated home route redirects to wallets", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);
  await page.goto("/");

  await expect(page).toHaveURL(/\/wallets$/);
  await expect(page.getByTestId("wallets-title")).toHaveText("Wallets");
  await expect(page.getByTestId("wallets-subtitle")).toHaveText(
    "Your accounts, in one place.",
  );

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
