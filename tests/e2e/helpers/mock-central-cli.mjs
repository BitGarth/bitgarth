import { startMockCentralServer } from "./mock-central.mjs";

async function main() {
  const portArg = process.argv.slice(2).find((arg) => arg.startsWith("--port="));
  const port = portArg ? Number(portArg.slice("--port=".length)) : 0;
  if (!Number.isFinite(port) || port < 0) {
    throw new Error(`Invalid port: ${portArg}`);
  }
  const mock = await startMockCentralServer({ port });

  console.log(`[mock-central] listening on ${mock.baseUrl}`);

  const shutdown = async () => {
    try {
      await mock.close();
    } finally {
      process.exit(0);
    }
  };

  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
