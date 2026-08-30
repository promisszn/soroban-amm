/**
 * GovernanceClient — typed client for the LP-governed fee-voting contract.
 *
 * Covers the public interface of contracts/governance/src/lib.rs.
 */

import {
  Contract,
  rpc as StellarRpc,
  nativeToScVal,
  scValToNative,
  xdr,
  Address,
} from "@stellar/stellar-sdk";
import type { NetworkConfig } from "./types.js";
import { simulateRead } from "./internal/simulate.js";

// ── Helpers ────────────────────────────────────────────────────────────────────

function addr(address: string): xdr.ScVal {
  return nativeToScVal(Address.fromString(address));
}

function u32(value: number): xdr.ScVal {
  return nativeToScVal(value, { type: "u32" });
}

// ── Types ──────────────────────────────────────────────────────────────────────

/** On-chain proposal status. */
export type ProposalStatus =
  | "Active"
  | "Pending"
  | "Queued"
  | "Executed"
  | "Defeated"
  | "Expired"
  | "Cancelled";

/** Vote choice passed to `vote`. */
export type VoteChoice = "For" | "Against" | "Abstain";

/** On-chain vote record for a voter. */
export type VoteRecord = "DidNotVote" | "VotedFor" | "VotedAgainst" | "VotedAbstain";

/** Governance configuration returned by `get_params`. */
export interface GovernanceParams {
  votingPeriodSecs: bigint;
  timelockSecs: bigint;
  quorumBps: bigint;
  minProposerStakeBps: bigint;
}

/** On-chain proposal data returned by `get_proposal`. */
export interface Proposal {
  id: number;
  proposer: string;
  snapshotTotalSupply: bigint;
  voteStart: bigint;
  voteEnd: bigint;
  executeAfter: bigint;
  expiresAt: bigint;
  votesFor: bigint;
  votesAgainst: bigint;
  votesAbstain: bigint;
  executed: boolean;
  cancelled: boolean;
  status: ProposalStatus;
}

// ── GovernanceClient ──────────────────────────────────────────────────────────

export class GovernanceClient {
  private readonly server: StellarRpc.Server;
  private readonly contract: Contract;
  private readonly networkPassphrase: string;

  constructor(config: NetworkConfig) {
    this.server = new StellarRpc.Server(config.rpcUrl);
    this.contract = new Contract(config.contractId);
    this.networkPassphrase = config.networkPassphrase;
  }

  get contractId(): string {
    return this.contract.contractId();
  }

  private async simulate(method: string, ...args: xdr.ScVal[]): Promise<xdr.ScVal> {
    return simulateRead(this.server, this.contract, this.networkPassphrase, method, args);
  }

  // ── Read-only methods ──────────────────────────────────────────────────────

  /** Returns the current governance configuration. */
  async getParams(): Promise<GovernanceParams> {
    const raw = await this.simulate("get_params");
    const native = scValToNative(raw) as Record<string, unknown>;
    return {
      votingPeriodSecs: BigInt(String(native.voting_period_secs ?? 0)),
      timelockSecs: BigInt(String(native.timelock_secs ?? 0)),
      quorumBps: BigInt(String(native.quorum_bps ?? 0)),
      minProposerStakeBps: BigInt(String(native.min_proposer_stake_bps ?? 0)),
    };
  }

  /** Returns the total number of proposals created so far. */
  async proposalCount(): Promise<number> {
    const raw = await this.simulate("proposal_count");
    return Number(scValToNative(raw));
  }

  /** Returns the on-chain data for `proposalId`. */
  async getProposal(proposalId: number): Promise<Proposal> {
    const raw = await this.simulate("get_proposal", u32(proposalId));
    const native = scValToNative(raw) as Record<string, unknown>;
    return {
      id: Number(native.id ?? proposalId),
      proposer: String(native.proposer ?? ""),
      snapshotTotalSupply: BigInt(String(native.snapshot_total_supply ?? 0)),
      voteStart: BigInt(String(native.vote_start ?? 0)),
      voteEnd: BigInt(String(native.vote_end ?? 0)),
      executeAfter: BigInt(String(native.execute_after ?? 0)),
      expiresAt: BigInt(String(native.expires_at ?? 0)),
      votesFor: BigInt(String(native.votes_for ?? 0)),
      votesAgainst: BigInt(String(native.votes_against ?? 0)),
      votesAbstain: BigInt(String(native.votes_abstain ?? 0)),
      executed: Boolean(native.executed),
      cancelled: Boolean(native.cancelled),
      status: "Active",
    };
  }

  /** Returns whether `voter` has voted on `proposalId`. */
  async hasVoted(proposalId: number, voter: string): Promise<boolean> {
    const raw = await this.simulate("has_voted", u32(proposalId), addr(voter));
    return Boolean(scValToNative(raw));
  }

  /** Returns the vote record for `voter` on `proposalId`. */
  async getVoteRecord(proposalId: number, voter: string): Promise<VoteRecord> {
    const raw = await this.simulate("get_vote_record", u32(proposalId), addr(voter));
    const native = scValToNative(raw);
    return String(native) as VoteRecord;
  }

  /** Returns the delegation target for `from`, or `null` if not delegated. */
  async getDelegate(from: string): Promise<string | null> {
    const raw = await this.simulate("get_delegate", addr(from));
    const native = scValToNative(raw);
    return native !== null && native !== undefined ? String(native) : null;
  }

  // ── Write-method parameter types ───────────────────────────────────────────

  /** Parameters for `propose(proposer, kind)` — returns the new proposal id. */
  proposeUpdateFeeParams(proposer: string, newFeeBps: bigint): xdr.ScVal[] {
    return [
      addr(proposer),
      nativeToScVal({ UpdateFee: newFeeBps }, { type: "map" }),
    ];
  }

  /** Parameters for `vote(voter, proposal_id, vote_choice)`. */
  voteParams(voter: string, proposalId: number, choice: VoteChoice): xdr.ScVal[] {
    return [addr(voter), u32(proposalId), nativeToScVal(choice)];
  }

  /** Parameters for `execute(proposal_id)`. */
  executeParams(proposalId: number): xdr.ScVal[] {
    return [u32(proposalId)];
  }

  /** Parameters for `cancel(proposal_id, caller)`. */
  cancelParams(proposalId: number, caller: string): xdr.ScVal[] {
    return [u32(proposalId), addr(caller)];
  }

  /** Parameters for `unlock_vote(proposal_id, voter)`. */
  unlockVoteParams(proposalId: number, voter: string): xdr.ScVal[] {
    return [u32(proposalId), addr(voter)];
  }

  /** Parameters for `delegate(from, to)`. */
  delegateParams(from: string, to: string): xdr.ScVal[] {
    return [addr(from), addr(to)];
  }
}
