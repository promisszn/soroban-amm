import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { parseAmountIn, parseDeadlineSeconds, parseSlippageBps } from "./config.js";

describe("parseSlippageBps", () => {
  it("rejects a negative value, naming SLIPPAGE_BPS", () => {
    assert.throws(() => parseSlippageBps("-50"), /SLIPPAGE_BPS/);
  });

  it("rejects 10000 (must be strictly less than 100%)", () => {
    assert.throws(() => parseSlippageBps("10000"), /SLIPPAGE_BPS/);
  });

  it("accepts a valid value", () => {
    assert.equal(parseSlippageBps("50"), 50n);
  });

  it("defaults to 50 when unset", () => {
    assert.equal(parseSlippageBps(undefined), 50n);
  });
});

describe("parseDeadlineSeconds", () => {
  it("rejects a non-numeric value, naming DEADLINE_SECONDS, not a raw BigInt/NaN error", () => {
    assert.throws(() => parseDeadlineSeconds("abc"), /DEADLINE_SECONDS/);
  });

  it("rejects zero and negative values", () => {
    assert.throws(() => parseDeadlineSeconds("0"), /DEADLINE_SECONDS/);
    assert.throws(() => parseDeadlineSeconds("-10"), /DEADLINE_SECONDS/);
  });

  it("accepts a valid value", () => {
    assert.equal(parseDeadlineSeconds("600"), 600);
  });

  it("defaults to 300 when unset", () => {
    assert.equal(parseDeadlineSeconds(undefined), 300);
  });
});

describe("parseAmountIn", () => {
  it("rejects zero, naming SWAP_AMOUNT_IN", () => {
    assert.throws(() => parseAmountIn("0"), /SWAP_AMOUNT_IN/);
  });

  it("rejects a negative value", () => {
    assert.throws(() => parseAmountIn("-1"), /SWAP_AMOUNT_IN/);
  });

  it("accepts a valid value", () => {
    assert.equal(parseAmountIn("100000"), 100000n);
  });

  it("defaults to 100000 when unset", () => {
    assert.equal(parseAmountIn(undefined), 100000n);
  });
});
