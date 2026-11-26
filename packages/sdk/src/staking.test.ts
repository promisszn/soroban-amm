import { describe, expect, it } from "vitest";
import { Networks } from "@stellar/stellar-sdk";
import { StakingClient } from "./staking.js";

const staking = new StakingClient({
  rpcUrl: "https://rpc.example.invalid",
  networkPassphrase: Networks.TESTNET,
  contractId: "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5SOAOPIFY6YQGAXE",
});

describe("StakingClient.boostMultiplierHuman", () => {
  // Keep in sync with BOOST_SCALE in contracts/staking/src/lib.rs.
  // MIN_BOOST = 10_000 and DEFAULT_MAX_BOOST = 25_000.
  it("converts the contract's scaled boost values to human multipliers", () => {
    expect(staking.boostMultiplierHuman(10_000n)).toBe(1);
    expect(staking.boostMultiplierHuman(25_000n)).toBe(2.5);
    expect(staking.boostMultiplierHuman(40_000n)).toBe(4);
  });
});
