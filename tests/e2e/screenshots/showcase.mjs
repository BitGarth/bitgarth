import { createHash } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

import { TEST_BTC_ADDRESS } from "../helpers/auth.mjs";
import { dailyPrice, PRICES } from "../helpers/mock-coingecko.mjs";

export const SHOWCASE_NOW = "2026-03-18T15:42:00Z";
export const SHOWCASE_ZONE = "Europe/Amsterdam";
export const SHOWCASE_USERNAME = "Garth";
export const OTHER_BTC_ADDRESS = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa";

export function daysBeforeShowcase(days) {
  return new Date(Date.parse(SHOWCASE_NOW) - days * 86_400_000).toISOString().slice(0, 10);
}

export function fixtureHash(label) {
  return createHash("sha256").update(label).digest("hex");
}

// Each spend consumes the preceding receipt and returns its change to the wallet.
const ledger = [
  ["2023-08-17T09:24:00Z", 803491, 10_438_640, 2410],
  ["2024-02-12T16:53:00Z", 830127, 8_260_000, 3142],
  ["2024-05-09T07:18:00Z", 842736, -3_215_000, 1560],
  ["2024-08-21T19:07:00Z", 857916, 6_137_800, 1978],
  ["2024-11-04T14:36:00Z", 868742, -2_749_000, 2140],
  ["2025-03-13T08:42:00Z", 887714, 4_490_500, 2310],
  ["2025-06-24T17:11:00Z", 902645, 3_781_200, 1425],
  ["2025-09-22T13:57:00Z", 915816, -1_580_000, 1890],
  ["2025-12-15T10:26:00Z", 928032, 5_436_700, 1235],
  ["2026-01-27T18:49:00Z", 934281, -3_654_200, 1740],
  ["2026-02-19T07:32:00Z", 937614, 2_843_900, 1690],
  ["2026-03-16T15:08:00Z", 941236, 224_330, 1328],
];

const peer = "1BoatSLRHtKNngkdXEeobR76b53LETtpyT";
const scripts = {
  [peer]: "76a9147680adec8eabcabac676be9e83854ade0bd22cdb88ac",
  [TEST_BTC_ADDRESS]: "00144be229a5c18db975e60f9d932eb0cb3bda4d0b2c",
  [OTHER_BTC_ADDRESS]: "76a91462e907b15cbf27d5425399ebf6f0fb50ebb88f1888ac",
};

function output(address, value) {
  return { scriptpubkey_address: address, scriptpubkey: scripts[address], value };
}

function addressHistory(address, entries) {
  const txs = [];
  let funded = 0;
  let spent = 0;
  for (const [at, height, amount, fee] of entries) {
    const txid = fixtureHash(`${address}/${at}`);
    const receive = amount > 0;
    const previous = txs.at(-1);
    const input = receive ? output(peer, amount + fee) : previous.vout[0];
    const outputs = receive ? [output(address, amount)] : [
      output(peer, -amount), output(address, input.value + amount - fee),
    ];
    funded += outputs.at(-1).value;
    if (!receive) spent += input.value;
    txs.push({
      txid, fee,
      vin: [{ txid: receive ? fixtureHash(`${txid}/input`) : previous.txid, vout: 0, prevout: input }],
      vout: outputs,
      status: {
        confirmed: true, block_height: height,
        block_hash: "0".repeat(19) + fixtureHash(`block/${height}`).slice(19),
        block_time: Date.parse(at) / 1000,
      },
    });
  }
  return { stats: { funded_txo_sum: funded, spent_txo_sum: spent, tx_count: txs.length }, txs: txs.reverse() };
}

export const SHOWCASE_BTC = addressHistory(TEST_BTC_ADDRESS, ledger);
export const OTHER_BTC = addressHistory(OTHER_BTC_ADDRESS, [
  ["2023-07-10T11:38:00Z", 798142, 20_000_000, 3100],
]);
export const SHOWCASE_TIP = 941527;

// Seed only the fresh, local capture instance. A stable user ID gives the
// normal login, navbar and settings page the same identicon on every run.
export async function seedShowcaseUser(projectDir) {
  const id = "01H7Y3MA805EA6B127HWKRDF8C";
  const created = "2023-08-16T08:15:00Z";
  const userDir = path.join(projectDir, "users", id, "data");
  await mkdir(userDir, { recursive: true });
  await writeFile(path.join(userDir, `u${id}.json`), JSON.stringify({ mode: "unencrypted-dev" }));
  const db = new DatabaseSync(path.join(projectDir, "app/data/app.db"));
  try {
    db.exec("BEGIN");
    db.prepare("INSERT INTO users (user_id, username, created_at, updated_at) VALUES (?, ?, ?, ?)")
      .run(id, SHOWCASE_USERNAME, created, created);
    db.prepare("INSERT INTO auth_credentials (credential_id, user_id, auth_method, is_primary, created_at) VALUES (?, ?, 'password', 1, ?)")
      .run(id, id, created);
    // Argon2id of the public E2E password (DEFAULT_TEST_PASSWORD).
    const salt = "Yml0Z2FydGgtc2hvd2Nhc2U";
    const hash = `$argon2id$v=19$m=19456,t=2,p=1$${salt}$QD7e5w87n8qDUDAzQSwUeW5evSvSG+ED+s6sf4SatDs`;
    db.prepare("INSERT INTO password_credentials (credential_id, password_hash, salt) VALUES (?, ?, ?)")
      .run(id, hash, salt);
    db.exec("COMMIT");
  } finally {
    db.close();
  }
}

// The live backfill starts at the server's real date. Supply the archived
// week directly from the same mock price series in the isolated price cache.
export function seedShowcasePrices(projectDir) {
  const db = new DatabaseSync(path.join(projectDir, "app/data/prices/prices.db"));
  try {
    const insert = db.prepare(`INSERT OR IGNORE INTO price_points
      (id, asset_id, quote_currency, price_time_utc, date_utc, price, provider,
       provider_asset_id, provider_quote_id, granularity, price_kind, license_scope,
       retrieved_at, created_at, updated_at)
      VALUES (?, 'bitcoin', 'EUR', ?, ?, ?, 'coingecko', 'bitcoin', 'eur',
              'daily', 'daily_point', 'public_keyless', ?, ?, ?)`);
    for (let days = 1; days <= 6; days++) {
      const date = daysBeforeShowcase(days);
      const at = `${date}T00:00:00Z`;
      insert.run(`coingecko:bitcoin:EUR:${at}:daily`, at, date,
        String(dailyPrice(PRICES.bitcoin, Date.parse(at) / 86_400_000)), SHOWCASE_NOW, SHOWCASE_NOW, SHOWCASE_NOW);
    }
  } finally {
    db.close();
  }
}
