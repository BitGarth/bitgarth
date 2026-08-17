import { expect, test } from "../helpers/mock-fixture.mjs";
import { registerViaUiAndExpectAuthenticated } from "../helpers/auth.mjs";

test("settings exposes update awareness and dismissible banner", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);

  await page.goto("/settings?section=system-info");

  await expect(page.getByRole("heading", { name: "Software updates" })).toBeVisible();

  await page.getByRole("button", { name: "Check now" }).click();

  await expect(page.getByText("Version 9.9.9 is available")).toBeVisible();
  const currentVersionLink = page
    .locator(".form-group", { hasText: "Current version" })
    .getByRole("link");
  const currentVersion = (await currentVersionLink.innerText()).trim();
  await expect(currentVersionLink).toHaveAttribute(
    "href",
    `https://bitgarth.app/releases.html#${currentVersion}`,
  );
  await expect(
    page
      .locator(".form-group", { hasText: "Latest version" })
      .getByRole("link"),
  ).toHaveAttribute("href", "https://bitgarth.app/releases.html#9.9.9");
  await expect(page.getByText(/You have version/)).toBeVisible();
  await expect(page.getByRole("button", { name: /copy upgrade command/i })).toBeVisible();

  await page.getByRole("button", { name: /remind me later/i }).click();

  await expect(page.getByText("Version 9.9.9 is available")).toBeHidden();
});
