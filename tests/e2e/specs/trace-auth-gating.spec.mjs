import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  getAuthenticatedUserId,
  registerViaUiAndExpectAuthenticated,
  saveMempoolBaseUrl,
  validateXpub,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";
import { startMockMempoolServer } from "../helpers/mock-mempool.mjs";
import {
  countHarTraceFiles,
  waitForHarTraceCountIncrease,
} from "../helpers/traces.mjs";

test("integration traces are gated by authentication", async ({ page }, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  expect(process.env.BITGARTH_PROJECT_DIR).toBeTruthy();

  const globalMempoolHarBefore = await countHarTraceFiles({ label: "mempool" });
  const loggedOutResponse = await validateXpub(page.request);
  expect(loggedOutResponse.status()).toBe(401);
  const globalMempoolHarAfter = await countHarTraceFiles({ label: "mempool" });
  expect(globalMempoolHarAfter).toBe(globalMempoolHarBefore);

  const mempoolStub = await startMockMempoolServer();
  try {
    await registerViaUiAndExpectAuthenticated(page);
    const userId = await getAuthenticatedUserId(page.request);
    await saveMempoolBaseUrl(page.request, mempoolStub.baseUrl);

    const userMempoolHarBefore = await countHarTraceFiles({
      userId,
      label: "mempool",
    });
    const loggedInResponse = await validateXpub(page.request);
    expect(loggedInResponse.ok()).toBeTruthy();

    const userMempoolHarAfter = await waitForHarTraceCountIncrease(
      userMempoolHarBefore,
      {
        userId,
        label: "mempool",
      },
    );
    expect(userMempoolHarAfter).toBeGreaterThan(userMempoolHarBefore);
    expect(mempoolStub.requestCount()).toBeGreaterThan(0);
  } finally {
    await mempoolStub.close();
  }

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
