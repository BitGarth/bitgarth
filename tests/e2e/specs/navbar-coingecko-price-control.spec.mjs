import { expect, test } from "../helpers/mock-fixture.mjs";
import {
  assertNoBrowserDiagnostics,
  registerViaUiAndExpectAuthenticated,
} from "../helpers/auth.mjs";
import {
  attachBrowserDiagnostics,
  markTestBoundary,
} from "../helpers/diagnostics.mjs";

test("navbar price control toggles, picks currency, and syncs with settings", async ({
  page,
}, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);

  await registerViaUiAndExpectAuthenticated(page);

  const navControl = page.getByTestId("nav-price-control");
  const navToggle = page.getByTestId("nav-price-fetching-toggle");
  const navCurrency = page.getByTestId("nav-price-currency-select");
  const navSwitch = navControl.locator(".nav-price-switch");

  await expect(navToggle).toBeVisible();
  await expect(navToggle).not.toBeChecked();
  await expect(navCurrency).toHaveCount(0);

  await navSwitch.click();
  await expect(navToggle).toBeChecked();
  await expect(navCurrency).toBeVisible();

  const navOptions = await navCurrency.locator("option").allInnerTexts();
  expect(navOptions).toEqual(["USD", "EUR", "GBP", "ZAR", "JPY", "CHF", "AUD", "CAD"]);

  await navCurrency.selectOption("EUR");
  await expect(navCurrency).toHaveValue("EUR");

  await expect(navControl).toHaveAttribute(
    "title",
    "When enabled, BitGarth requests prices for your assets and selected currency.",
  );

  await page.goto("/settings?section=regional");
  await expect(page.locator("#currency-selector")).toHaveValue("EUR");

  await page.goto("/settings?section=digital-assets");
  await expect(page.getByTestId("price-fetching-toggle")).toBeChecked();
  await expect(
    page.getByText("When enabled, BitGarth requests prices for your assets and selected currency."),
  ).toBeVisible();

  await navSwitch.click();
  await expect(page.getByTestId("price-fetching-toggle")).not.toBeChecked();
  await expect(page.getByText("CoinGecko price fetching disabled.")).toBeVisible();

  await navSwitch.click();
  await expect(page.getByTestId("price-fetching-toggle")).toBeChecked();
  await expect(page.getByText("CoinGecko price fetching enabled.")).toBeVisible();

  await page.getByTestId("price-fetching-toggle").uncheck();
  await expect(navToggle).not.toBeChecked();
  await expect(navCurrency).toHaveCount(0);

  assertNoBrowserDiagnostics(diagnostics);
  await markTestBoundary(testInfo, "END");
});
