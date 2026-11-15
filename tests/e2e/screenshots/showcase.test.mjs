import assert from "node:assert/strict";
import test from "node:test";
import { TEST_BTC_ADDRESS } from "../helpers/auth.mjs";
import { SHOWCASE_BTC, SHOWCASE_NOW, SHOWCASE_TIP } from "./showcase.mjs";

test("the showcase wallet's spendable outputs reconcile to its displayed Bitcoin balance", () => {
  const { txs, stats } = SHOWCASE_BTC;
  const unspent = new Map();
  let funded = 0;
  let spent = 0;
  assert.equal(stats.tx_count, 12);
  assert.equal(txs.length, stats.tx_count);
  for (const tx of txs.toReversed()) {
    assert.match(tx.txid, /^[a-f0-9]{64}$/);
    assert.ok(tx.status.block_height <= SHOWCASE_TIP);
    assert.ok(tx.status.block_time < Date.parse(SHOWCASE_NOW) / 1000);
    let inputValue = 0;
    for (const input of tx.vin) {
      inputValue += input.prevout.value;
      if (input.prevout.scriptpubkey_address === TEST_BTC_ADDRESS) {
        const ref = `${input.txid}:${input.vout}`;
        assert.equal(unspent.get(ref), input.prevout.value);
        unspent.delete(ref);
        spent += input.prevout.value;
      }
    }
    assert.equal(inputValue, tx.fee + tx.vout.reduce((sum, out) => sum + out.value, 0));
    for (const [index, out] of tx.vout.entries()) {
      if (out.scriptpubkey_address === TEST_BTC_ADDRESS) {
        funded += out.value;
        unspent.set(`${tx.txid}:${index}`, out.value);
      }
    }
  }
  assert.equal(funded, stats.funded_txo_sum);
  assert.equal(spent, stats.spent_txo_sum);
  assert.equal(funded - spent, 30_407_540);
});
