import { test as base } from "@playwright/test";
import { startMockServers } from "./mock-servers.mjs";

export { expect } from "@playwright/test";

export const test = base.extend({
  mempoolAddressData: [{}, { option: true, scope: "worker" }],
  mockServers: [
    async ({ mempoolAddressData }, use) => {
      const mocks = await startMockServers({ mempoolAddressData });
      await use({
        etherscan: mocks.etherscan,
        mempool: mocks.mempool,
        adminUrl: mocks.adminUrl,
      });
      await mocks.close();
    },
    { scope: "worker" },
  ],
});
