import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

test("sidebar contact footer shows the email and opens the PGP key", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  // Copying uses the clipboard API, which a real browser allows on a user
  // gesture; grant it so the write doesn't reject in headless Chromium.
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);

  await registerViaUiAndExpectAuthenticated(page);

  // The email is shown directly in the sidebar footer.
  const emailButton = page.getByTestId("sidebar-contact-email");
  await expect(emailButton).toContainText("hello@bitgarth.app");

  // Clicking it copies (copy-first), reflected in the button's title.
  await expect(emailButton).toHaveAttribute("title", "Copy email address");
  await emailButton.click();
  await expect(emailButton).toHaveAttribute("title", "Copied");

  // "Compose" is the mailto escape hatch.
  await expect(
    page.getByRole("link", { name: "Compose" }),
  ).toHaveAttribute("href", "mailto:hello@bitgarth.app");

  // "PGP key" opens the PGP modal with the key shown directly (not blank).
  const dialog = page.getByRole("dialog", { name: "PGP public key" });
  await expect(dialog).toHaveCount(0);
  await page.getByTestId("sidebar-contact-pgp").click();
  await expect(dialog).toBeVisible();
  await expect(dialog.locator(".contact-pgp-fp")).toContainText(
    "40F4 6DAB EDD9 A5F4 4047",
  );
  await expect(dialog.getByText("BEGIN PGP PUBLIC KEY BLOCK")).toBeVisible();
  const ascLink = dialog.getByRole("link", { name: "Download .asc" });
  await expect(ascLink).toHaveAttribute("href", /hello-bitgarth-pubkey.*\.asc/);

  // Esc dismisses after focus moves away from the initially focused close button (§9.12).
  await dialog.locator(".contact-pgp-note").click();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
