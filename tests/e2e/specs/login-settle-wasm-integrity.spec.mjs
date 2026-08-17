import { expect, test } from "../helpers/mock-fixture.mjs";
import { registerViaUiAndExpectAuthenticated } from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

// Regression guard for an intermittent WASM `RuntimeError: unreachable`.
//
// Registering navigates to /wallets while that page is still resolving its
// `use_server_future`s. Anything that changes the NavBar layout during that
// settle window (previously: the update-status resource restarting and
// re-suspending the layout, then the update banner appearing) makes
// dioxus-core 0.7.9 reclaim an element id twice. The arena corruption is
// silent most of the time and traps the module occasionally, which is what
// made the wallet specs flaky.
//
// `cannot reclaim ElementId(..)` is the deterministic precursor, so assert on
// that rather than on the rare trap. It reaches the browser console through
// dioxus's tracing layer as a `console.log`, which is why
// `assertNoBrowserDiagnostics` never saw it.
test("registration settles without corrupting the WASM element arena", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START login-settle-wasm-integrity");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);
  await expect(page).toHaveURL(/\/wallets$/);

  // Outlive the deferred automatic update check so its banner lands inside the
  // observed window.
  await expect(page.locator(".upgrade-notice")).toBeVisible({ timeout: 15_000 });

  expect(
    diagnostics.arenaErrors,
    `Dioxus element-arena errors:\n${diagnostics.arenaErrors.join("\n")}`,
  ).toEqual([]);
  expect(
    diagnostics.pageErrors,
    `Page errors:\n${diagnostics.pageErrors.join("\n")}`,
  ).toEqual([]);

  await markTestBoundary(testInfo, "END login-settle-wasm-integrity");
});
