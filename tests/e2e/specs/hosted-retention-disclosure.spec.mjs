import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  waitForGeneratedUsername,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

test("self-hosted (docker) registration shows no inactivity disclosure", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await page.goto("/register");

  // Wait for WASM hydration to complete before asserting absence — a
  // server-side-only element could theoretically appear during hydration.
  await waitForGeneratedUsername(page);

  await expect(page.getByTestId("legal-acknowledgement-checkbox")).toBeVisible();
  await expect(page.getByTestId("hosted-retention-disclosure")).toHaveCount(0);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
