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

// The "Enable update checks" toggle defaults on, so an authenticated session
// must run a version check on its own — without the user pressing "Check now".
// Mock central returns latest=9.9.9; a successful auto-check persists the
// timestamp, so "Last checked" must stop reading "Not checked yet".
test("update check runs automatically after login (no Check now click)", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  // Catch the auto-fired refresh request the NavBar effect makes on login.
  const refreshHit = page.waitForRequest(
    (req) =>
      req.url().includes("/_app/updates/refresh") && req.method() === "POST",
    { timeout: 15_000 },
  );

  await page.goto("/register");
  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);

  await refreshHit; // the check fired without any "Check now" interaction

  await page.goto("/settings?section=system-info");

  const lastChecked = page
    .locator(".form-group", { hasText: "Last checked" })
    .locator(".form-value");
  // Auto-check persists a timestamp; poll until the resource reflects it.
  await expect(lastChecked).not.toHaveText("Not checked yet", { timeout: 15_000 });

  const latest = page
    .locator(".form-group", { hasText: "Latest version" })
    .locator(".form-value");
  await expect(latest).toContainText("9.9.9");

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
