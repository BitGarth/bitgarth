import os from "node:os";
import { expect, test } from "../helpers/mock-fixture.mjs";
import { assertNoBrowserDiagnostics, registerViaUiAndExpectAuthenticated } from "../helpers/auth.mjs";
import { attachBrowserDiagnostics, markTestBoundary } from "../helpers/diagnostics.mjs";

const central = "http://127.0.0.1:8082";
const release = "https://github.com/BitGarth/bitgarth/releases/tag/v9.9.9";

test.skip(process.env.BITGARTH_E2E_CHANNEL !== "web", "Run with BITGARTH_E2E_CHANNEL=web");

test("web update notice uses canonical release and valid v2 metadata", async ({ page, request }, testInfo) => {
  await markTestBoundary(testInfo, "START");
  const diagnostics = await attachBrowserDiagnostics(page, testInfo);
  const reset = await request.post(`${central}/__mock/reset`);
  expect(reset.ok()).toBeTruthy();
  await seed({ channels: { default: { latest: "v9.9.9" } } });

  try {
    await registerViaUiAndExpectAuthenticated(page);
    const banner = page.getByRole("region", { name: "Application update available" });
    await expect(banner).toBeVisible();
    await expect(banner.getByRole("link", { name: "Installation instructions" }))
      .toHaveAttribute("href", "https://bitgarth.app/#install");
    const releaseLink = banner.getByRole("link", { name: "View release" });
    await expect(releaseLink).toHaveAttribute("href", release);
    await expect(releaseLink).toHaveAttribute("target", "_blank");
    await expect(releaseLink).toHaveAttribute("rel", "noopener noreferrer");
    await expect(banner.getByRole("button", { name: "Copy upgrade command" })).toHaveCount(0);
    await banner.getByRole("button", { name: "Remind me later" }).click();
    await expect(banner).toBeHidden();

    await expect.poll(async () => (await mockStatus()).latestAppVersionRequests.length).toBeGreaterThan(0);
    const calls = (await mockStatus()).latestAppVersionRequests;
    expect(calls.every(({ path }) => path === "/api/v2/latest-app-version")).toBeTruthy();
    expect(calls.at(-1).channel).toBe("web");
    expect(calls.at(-1).version).toBeTruthy();
    expect(calls.at(-1).platform).toBe(`${os.platform() === "darwin" ? "macos" : os.platform()}/${os.arch() === "x64" ? "x86_64" : os.arch() === "arm64" ? "aarch64" : os.arch()}`);

    const original = await appStatus();
    await seed({ channels: { default: { latest: "v9.9.10", release_url: "https://attacker.example/release" } } });
    await refresh();
    await page.reload();
    await expect(page.getByRole("region", { name: "Application update available" })
      .getByRole("link", { name: "View release" }))
      .toHaveAttribute("href", "https://github.com/BitGarth/bitgarth/releases/tag/v9.9.10");
    const updated = await appStatus();
    expect(updated.latest).toBe("v9.9.10");

    await seed({ channels: { web: { latest: "not-a-version" }, default: { latest: "v9.9.11" } } });
    await refresh();
    const invalid = await appStatus();
    expect(invalid.latest).toBe(updated.latest);
    expect(invalid.last_checked_at).toBe(updated.last_checked_at);

    await seed({ channels: {} });
    await refresh();
    const missing = await appStatus();
    expect(missing.latest).toBe(updated.latest);
    expect(missing.last_checked_at).toBe(updated.last_checked_at);

    const disabled = await page.request.post("/_app/updates/checks-enabled", { data: { enabled: false } });
    expect(disabled.ok()).toBeTruthy();
    const before = (await mockStatus()).latestAppVersionRequests.length;
    await refresh();
    await expect.poll(async () => (await mockStatus()).latestAppVersionRequests.length).toBe(before + 1);
    await page.reload();
    await expect(page.getByRole("region", { name: "Application update available" })).toHaveCount(0);
    expect(original.latest).toBe("v9.9.9");
    assertNoBrowserDiagnostics(diagnostics);
    await markTestBoundary(testInfo, "END");
  } finally {
    await page.request.post("/_app/updates/checks-enabled", { data: { enabled: true } });
    await request.post(`${central}/__mock/reset`);
  }

  async function seed(latestAppVersions) {
    const response = await request.post(`${central}/__mock/scenario`, { data: { latestAppVersions } });
    expect(response.ok()).toBeTruthy();
  }
  async function mockStatus() {
    return (await request.get(`${central}/__mock/status`)).json();
  }
  async function appStatus() {
    const response = await page.request.get("/_app/updates/status");
    expect(response.ok()).toBeTruthy();
    return response.json();
  }
  async function refresh() {
    const response = await page.request.post("/_app/updates/refresh", { data: { force: true } });
    expect(response.ok()).toBeTruthy();
  }
});
