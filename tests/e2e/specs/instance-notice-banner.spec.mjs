import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  configureMockServers,
  registerViaUiAndExpectAuthenticated,
  waitForGeneratedUsername,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

test("instance notice banner renders on the unauthenticated landing page", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await page.goto("/login");

  const banner = page.locator(".instance-notice");
  await expect(banner).toBeVisible();
  await expect(banner).toHaveAttribute("role", "status");
  await expect(banner).toContainText("E2E demo notice");

  const externalLink = banner.locator('a[href="https://bitgarth.app/"]');
  await expect(externalLink).toBeVisible();
  await expect(externalLink).toHaveAttribute("target", "_blank");
  await expect(externalLink).toHaveAttribute("rel", "noopener noreferrer");

  const mailtoLink = banner.locator('a[href="mailto:hello@bitgarth.app"]');
  await expect(mailtoLink).toBeVisible();
  await expect(mailtoLink).not.toHaveAttribute("target", "_blank");
  await expect(mailtoLink).toHaveAttribute("rel", "noopener noreferrer");

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

// Regression: `BuildDriftState` and `InstanceNoticeState` were both bare
// `Signal<Option<String>>` aliases, so they collided in Dioxus's type-keyed
// context. After WASM hydration the drift watcher nulled the shared signal and
// wiped this banner. A plain `toBeVisible()` passes on the SSR paint before
// hydration runs, so this case explicitly waits for client hydration first.
test("instance notice banner survives WASM hydration", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await page.goto("/register");

  // Client-side username generation only runs once hydration completes.
  await waitForGeneratedUsername(page);

  const banner = page.locator(".instance-notice");
  await expect(banner).toBeVisible();
  await expect(banner).toContainText("E2E demo notice");

  // The shared-context bug also rendered the drift strip with the notice html
  // as its "server build"; with distinct contexts it must stay absent.
  await expect(page.locator(".update-notice")).toHaveCount(0);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});

test("instance notice banner persists across login boundary", async ({
  page,
  mockServers,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await page.goto("/register");
  const bannerOnRegister = page.locator(".instance-notice");
  await expect(bannerOnRegister).toBeVisible();

  await registerViaUiAndExpectAuthenticated(page);
  await configureMockServers(page.request, mockServers);

  const bannerAfterAuth = page.locator(".instance-notice");
  await expect(bannerAfterAuth).toBeVisible();
  await expect(bannerAfterAuth).toContainText("E2E demo notice");

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
