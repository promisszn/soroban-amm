import { describe, expect, it } from "vitest";
import { Address, Networks, nativeToScVal, xdr } from "@stellar/stellar-sdk";
import { IncentiveCampaignsClient } from "./incentives.js";
import { ContractDecodeError } from "./internal/decode.js";

const CONTRACT_ID = "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5SOAOPIFY6YQGAXE";
const POOL = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";
const LP_TOKEN = "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ";
const REWARD_TOKEN = "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5SOAOPIFY6YQGAXE";

const client = new IncentiveCampaignsClient({
  rpcUrl: "https://rpc.example.invalid",
  networkPassphrase: Networks.TESTNET,
  contractId: CONTRACT_ID,
});

const u64 = (value: bigint) => nativeToScVal(value, { type: "u64" });
const i128 = (value: bigint) => nativeToScVal(value, { type: "i128" });
const address = (value: string) => nativeToScVal(Address.fromString(value));

function campaignScVal(omit?: string): xdr.ScVal {
  const fields: Record<string, xdr.ScVal> = {
    id: u64(7n),
    pool: address(POOL),
    lp_token: address(LP_TOKEN),
    reward_token: address(REWARD_TOKEN),
    start_time: u64(1_700_000_000n),
    end_time: u64(1_800_000_000n),
    reward_rate: i128(25n),
    active: nativeToScVal(true),
    total_distributed: i128(300n),
    funding_amount: i128(1_000n),
    acc_reward_per_share: i128(42n),
    last_update_time: u64(1_750_000_000n),
  };
  if (omit) delete fields[omit];
  return xdr.ScVal.scvMap(
    Object.entries(fields).map(
      ([key, val]) => new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol(key), val })
    )
  );
}

function mockReturn(value: xdr.ScVal): void {
  const server = (client as unknown as { server: Record<string, unknown> }).server;
  server.simulateTransaction = async () => ({ result: { retval: value } });
}

describe("IncentiveCampaignsClient campaign decoding", () => {
  it("decodes every snake_case contract field to the camelCase SDK shape", async () => {
    mockReturn(campaignScVal());

    await expect(client.getCampaign(7n)).resolves.toEqual({
      id: 7n,
      pool: POOL,
      lpToken: LP_TOKEN,
      rewardToken: REWARD_TOKEN,
      startTime: 1_700_000_000n,
      endTime: 1_800_000_000n,
      rewardRate: 25n,
      active: true,
      totalDistributed: 300n,
      fundingAmount: 1_000n,
      accRewardPerShare: 42n,
      lastUpdateTime: 1_750_000_000n,
    });
  });

  it("uses the same decoder for every active campaign", async () => {
    mockReturn(xdr.ScVal.scvVec([campaignScVal()]));

    const campaigns = await client.getActiveCampaignsPaginated();
    expect(campaigns).toHaveLength(1);
    expect(campaigns[0].accRewardPerShare).toBe(42n);
    expect(campaigns[0].lastUpdateTime).toBe(1_750_000_000n);
    expect(campaigns[0].startTime).toBe(1_700_000_000n);
  });

  it("rejects a campaign with a missing required field", async () => {
    mockReturn(campaignScVal("reward_rate"));

    await expect(client.getCampaign(7n)).rejects.toThrow(ContractDecodeError);
    await expect(client.getCampaign(7n)).rejects.toThrow("Campaign.reward_rate");
  });
});
