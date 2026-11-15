import { expect, test } from "../helpers/mock-fixture.mjs";
import { registerViaUiAndExpectAuthenticated } from "../helpers/auth.mjs";

test.use({ timezoneId: "Pacific/Auckland" });

test("searching timezones preserves the saved choice until a timezone is selected", async ({ page }) => {
  await registerViaUiAndExpectAuthenticated(page);
  const savedTimezone = async () => {
    const response = await page.request.get("/_app/user/settings");
    expect(response.ok()).toBeTruthy();
    return (await response.json()).timezone;
  };
  await expect.poll(savedTimezone).toBe("Pacific/Auckland");
  const seeded = await page.request.post("/_app/user/settings/timezone", {
    data: { timezone: "US/Eastern" },
  });
  expect(seeded.ok()).toBeTruthy();
  await page.goto("/settings");

  const search = page.getByRole("searchbox", { name: "Search timezones" });
  const select = page.getByRole("combobox", { name: "Select timezone" });
  await expect(select).toHaveValue("US/Eastern");
  await search.fill("no such zone");
  await expect(page.getByText("No matching timezones.")).toBeVisible();
  await expect(select.locator("option")).toHaveCount(1);
  await expect(select).toHaveValue("US/Eastern");

  await search.fill("nEw yOrK");
  await expect(select.locator("option")).toHaveText(["US/Eastern", "America/New_York"]);
  await expect(select).toHaveValue("US/Eastern");
  expect(await savedTimezone()).toBe("US/Eastern");
  await select.selectOption("America/New_York");
  await expect.poll(savedTimezone).toBe("America/New_York");
  await expect(select).toHaveValue("America/New_York");
  await expect(search).toHaveValue("");
  await expect(select.getByRole("option", { name: "US/Eastern", exact: true })).toHaveCount(0);

  await search.fill("no such zone");
  await page.getByRole("button", { name: "Use browser timezone (Pacific/Auckland)" }).click();
  await expect.poll(savedTimezone).toBe("Pacific/Auckland");
  await expect(select).toHaveValue("Pacific/Auckland");
  await expect(search).toHaveValue("");
});
