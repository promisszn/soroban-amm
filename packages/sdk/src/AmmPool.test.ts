/**
 * Tests for AmmPool's contract-error decoder — Issue #831.
 *
 * Soroban RPC reports contract errors as `Error(Contract, #N)`, so the decoder
 * must map the numeric discriminant to an `AmmErrors` entry rather than
 * substring-matching English text (which never matched a real error).
 */

import { describe, it, expect } from "vitest";

import { AmmContractError, decodeError } from "./AmmPool.js";
import { AmmErrors, AmmErrorNames } from "./types.js";

describe("decodeError", () => {
  it("maps Error(Contract, #6) to the PAUSED entry", () => {
    const err = decodeError(
      new Error("host invocation failed: Error(Contract, #6)")
    );
    expect(err).toBeInstanceOf(AmmContractError);
    const contractErr = err as AmmContractError;
    expect(contractErr.code).toBe(6);
    expect(contractErr.name).toBe("Paused");
    expect(contractErr.message).toBe(`AMM error: ${AmmErrors[6]}`);
    expect(contractErr.message).toContain("contract is paused");
  });

  it.each([
    [4, "DeadlineExceeded", "deadline exceeded"],
    [5, "SlippageExceeded", "slippage exceeded"],
    [7, "Unauthorized", "unauthorized"],
    [11, "InsufficientLiquidity", "insufficient liquidity"],
    [18, "FlashLoanRepaymentFailed", "flash loan repayment failed"],
  ])("maps Error(Contract, #%i) to %s", (code, name, text) => {
    const err = decodeError(new Error(`Error(Contract, #${code})`));
    expect(err).toBeInstanceOf(AmmContractError);
    const contractErr = err as AmmContractError;
    expect(contractErr.code).toBe(code);
    expect(contractErr.name).toBe(name);
    expect(contractErr.message).toBe(`AMM error: ${text}`);
  });

  it("decodes every discriminant declared in AmmErrors", () => {
    for (const key of Object.keys(AmmErrors)) {
      const code = Number(key);
      const err = decodeError(new Error(`Error(Contract, #${code})`));
      expect(err).toBeInstanceOf(AmmContractError);
      expect((err as AmmContractError).code).toBe(code);
    }
  });

  it("tolerates whitespace variations in the RPC error format", () => {
    for (const raw of [
      "Error(Contract, #6)",
      "Error(Contract,#6)",
      "Error( Contract , #6 )",
      "Error  (  Contract  ,  #6  )",
    ]) {
      const err = decodeError(new Error(raw));
      expect((err as AmmContractError).code).toBe(6);
    }
  });

  it("preserves the raw RPC message on the decoded error", () => {
    const raw = "simulation failed: Error(Contract, #9) at ledger 42";
    const err = decodeError(new Error(raw)) as AmmContractError;
    expect(err.rawMessage).toBe(raw);
  });

  it("accepts a bare string as well as an Error", () => {
    const err = decodeError("Error(Contract, #6)");
    expect((err as AmmContractError).code).toBe(6);
  });

  it("falls back to the raw message when no discriminant is present", () => {
    const err = decodeError(new Error("connection refused"));
    expect(err).not.toBeInstanceOf(AmmContractError);
    expect(err.message).toBe("AMM error: connection refused");
  });

  it("does not match on descriptive text alone", () => {
    // The old decoder matched this substring; a real RPC error never looks
    // like this, so it must fall through to the raw-message branch.
    const err = decodeError(new Error("the contract is paused"));
    expect(err).not.toBeInstanceOf(AmmContractError);
    expect(err.message).toBe("AMM error: the contract is paused");
  });

  it("reports unknown discriminants without inventing a mapping", () => {
    const err = decodeError(new Error("Error(Contract, #999)"));
    expect(err).not.toBeInstanceOf(AmmContractError);
    expect(err.message).toContain("unknown contract error #999");
  });
});

describe("AmmErrors", () => {
  it("covers all 18 AmmError discriminants from contracts/amm/src/lib.rs", () => {
    const codes = Object.keys(AmmErrors).map(Number).sort((a, b) => a - b);
    expect(codes).toEqual(Array.from({ length: 18 }, (_, i) => i + 1));
  });

  it("has a symbolic name for every discriminant", () => {
    expect(Object.keys(AmmErrorNames).sort()).toEqual(Object.keys(AmmErrors).sort());
  });

  it("mirrors the Rust variant names exactly", () => {
    // Transcribed from `pub enum AmmError` in contracts/amm/src/lib.rs:61.
    expect(AmmErrorNames).toEqual({
      1: "AlreadyInitialized",
      2: "InvalidFeeBps",
      3: "InsufficientShares",
      4: "DeadlineExceeded",
      5: "SlippageExceeded",
      6: "Paused",
      7: "Unauthorized",
      8: "ZeroAmount",
      9: "InvalidToken",
      10: "EmptyPool",
      11: "InsufficientLiquidity",
      12: "NoPendingAdmin",
      13: "WrongAdmin",
      14: "Reentrant",
      15: "CircuitBreaker",
      16: "FotSlippage",
      17: "OracleDeviationExceeded",
      18: "FlashLoanRepaymentFailed",
    });
  });
});
