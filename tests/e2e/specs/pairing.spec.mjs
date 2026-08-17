import { createHash } from "node:crypto";

import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  loginViaUi,
  logoutViaUserMenu,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

const VERIFIER_DOMAIN = Buffer.from("bitgarth-client-key-verifier-v1\0");

function clientCredential(byte) {
  const raw = Buffer.alloc(32, byte);
  return {
    key: raw.toString("base64url"),
    verifier: createHash("sha256")
      .update(VERIFIER_DOMAIN)
      .update(raw)
      .digest("base64url"),
  };
}

test("paired client approval, listing, and revocation", async ({ page }, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  const credentials = await registerViaUiAndExpectAuthenticated(page);
  await logoutViaUserMenu(page);

  const client = clientCredential(81);
  const start = await page.request.post("/api/v1/pairings", {
    data: {
      client_name: "E2E paired client",
      key_verifier: client.verifier,
      permissions: ["balances_read"],
    },
  });
  const startBody = await start.text();
  expect(start.ok(), `pairing start ${start.status()}: ${startBody}`).toBeTruthy();
  const pairing = JSON.parse(startBody);

  await page.goto(`/pair?code=${encodeURIComponent(pairing.code)}`);
  await expect(
    page.getByRole("heading", { name: "Sign in to review this pairing" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/pair\/[^/]+\/login$/);
  await loginViaUi(page, credentials.username, credentials.password);

  await expect(
    page.getByRole("heading", { name: "Review Client Key pairing" }),
  ).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText("E2E paired client", { exact: true })).toBeVisible();
  await expect(page.getByText(pairing.code, { exact: true })).toBeVisible();
  await page
    .getByLabel("I confirm this code matches the code shown by the initiating CLI.")
    .check();
  let releaseApproval;
  const approvalReleased = new Promise((resolve) => {
    releaseApproval = resolve;
  });
  let markApprovalStarted;
  const approvalStarted = new Promise((resolve) => {
    markApprovalStarted = resolve;
  });
  await page.route("**/_app/pairings/approve", async (route) => {
    markApprovalStarted();
    await approvalReleased;
    await route.continue();
  });

  const approve = page.getByRole("button", { name: "Approve" });
  const deny = page.getByRole("button", { name: "Deny" });
  await approve.click({ noWaitAfter: true });
  await approvalStarted;
  await expect(approve).toBeDisabled();
  await expect(deny).toBeDisabled();
  releaseApproval();

  const waiting = page
    .getByRole("status")
    .filter({ hasText: "Pairing approved. Waiting for the CLI to finish" });
  await expect(waiting).toBeVisible();
  await expect(page.getByRole("button", { name: "Approve" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Deny" })).toHaveCount(0);
  await expect(page.getByLabel("Client expiry (UTC, optional)")).toHaveCount(0);

  const claim = await page.request.post(
    `/api/v1/pairings/${encodeURIComponent(pairing.pairing_id)}/claim`,
    { headers: { Authorization: `Bearer ${client.key}` } },
  );
  expect(claim.ok()).toBeTruthy();
  expect((await claim.json()).status).toBe("active");

  const success = page
    .getByRole("status")
    .filter({ hasText: "Pairing successful" });
  await expect(success).toContainText("E2E paired client", { timeout: 15_000 });
  const pairedClientsLink = page.getByRole("link", { name: "View paired clients" });
  await expect(pairedClientsLink).toHaveAttribute(
    "href",
    "/settings?section=account",
  );
  await pairedClientsLink.click();
  await expect(page).toHaveURL(/\/settings\?section=account$/);

  const card = page.getByTestId("paired-clients-card");
  await expect(card).toBeVisible();
  await expect(card.getByText("E2E paired client", { exact: true })).toBeVisible();
  await expect(card.getByText("balances_read", { exact: true })).toBeVisible();
  await expect(card.getByText("Never", { exact: true })).toBeVisible();

  await card
    .getByRole("button", { name: "Revoke paired client E2E paired client" })
    .click();
  const dialog = page.getByRole("dialog", { name: "Revoke Paired Client" });
  await expect(dialog).toContainText("immediately lose CLI access");
  await dialog
    .getByRole("button", { name: "Revoke paired client E2E paired client" })
    .click();
  await expect(dialog).toBeHidden();
  await expect(card.getByText(/^Revoked /)).toBeVisible();

  const rejected = await page.request.post(
    `/api/v1/pairings/${encodeURIComponent(pairing.pairing_id)}/claim`,
    { headers: { Authorization: `Bearer ${client.key}` } },
  );
  expect(rejected.status()).toBe(401);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
