import { AmmPool, FactoryClient, TokenClient } from "@soroban-amm/sdk";
import { parseAmountIn, parseDeadlineSeconds, parseSlippageBps } from "./config.js";
import { makeKeypairSigner } from "./signer.js";

const rpcUrl = process.env.STELLAR_RPC_URL ?? "https://soroban-testnet.stellar.org";
const networkPassphrase = process.env.STELLAR_NETWORK_PASSPHRASE ?? "Test SDF Network ; September 2015";
const poolId = required("AMM_CONTRACT_ID");
const tokenInId = required("TOKEN_IN_CONTRACT_ID");
const sourceAddress = required("SOURCE_ADDRESS");
const lpTokenId = process.env.LP_TOKEN_CONTRACT_ID;

function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required; see examples/client/README.md`);
  return value;
}

function config(contractId: string) {
  return { rpcUrl, networkPassphrase, contractId };
}

async function main(): Promise<void> {
  const pool = new AmmPool(config(poolId));
  const token = new TokenClient(config(tokenInId));
  const factoryId = process.env.FACTORY_CONTRACT_ID;
  const factory = factoryId ? new FactoryClient(config(factoryId)) : undefined;

  let amountIn: bigint;
  let deadlineSeconds: number;
  let slippageBps: bigint;
  try {
    amountIn = parseAmountIn(process.env.SWAP_AMOUNT_IN);
    deadlineSeconds = parseDeadlineSeconds(process.env.DEADLINE_SECONDS);
    slippageBps = parseSlippageBps(process.env.SLIPPAGE_BPS);
  } catch (error: unknown) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`Invalid configuration: ${message}`);
    process.exitCode = 1;
    return;
  }
  const deadline = BigInt(Math.floor(Date.now() / 1000) + deadlineSeconds);

  // Build a signer from SIGNING_SECRET_KEY. Set DRY_RUN=1 to skip writes and
  // only print the reviewed parameters — useful when no funded key is available.
  const dryRun = process.env.DRY_RUN === "1";
  const signer = dryRun ? null : makeKeypairSigner();

  if (signer && await signer.publicKey() !== sourceAddress) {
    throw new Error("SIGNING_SECRET_KEY does not match SOURCE_ADDRESS");
  }

  // 1. Read pool info to confirm the pair and current reserves.
  const info = await pool.getInfo();
  console.log("Pool", poolId, "tokens", info.tokenA, info.tokenB);

  // 2. simulateSwap quotes the swap off-chain. bigint preserves token precision
  //    and price impact is computed from the same constant-product formula the
  //    contract uses, so the quote is accurate before any network round-trip.
  const quote = await pool.simulateSwap(tokenInId, amountIn);
  const minAmountOut = quote.amountOut * (10_000n - slippageBps) / 10_000n;
  console.log("Quote", {
    amountIn: quote.amountIn,
    amountOut: quote.amountOut,
    priceImpactBps: quote.priceImpactBps,
    minAmountOut,
    deadline,
  });

  // 3. Submit the swap via AmmPool.swap. The SDK simulates the transaction,
  //    assembles the Soroban auth entries, signs it via the supplied signer,
  //    and polls until the transaction is confirmed.
  if (signer) {
    const { returnValue: amountOut, hash } = await pool.swap(
      { trader: sourceAddress, tokenIn: tokenInId, amountIn, minAmountOut, deadline },
      signer
    );
    console.log("Swap confirmed", { amountOut, hash });
  } else {
    console.log("DRY_RUN: swap would submit", { trader: sourceAddress, tokenIn: tokenInId, amountIn, minAmountOut, deadline });
  }

  // 4. Add liquidity — opt-in via ADD_LIQUIDITY=1 so the default run is a swap only.
  const tokenSymbol = await token.symbol();
  if (process.env.ADD_LIQUIDITY === "1") {
    const addDeadline = BigInt(Math.floor(Date.now() / 1000) + deadlineSeconds);
    const addParams = {
      provider: sourceAddress,
      amountA: amountIn,
      amountB: amountIn,          // symmetric deposit; adjust per pool pair
      minShares: 1n,
      deadline: addDeadline,
    };
    if (signer) {
      const { returnValue: shares, hash } = await pool.addLiquidity(addParams, signer);
      console.log("Add-liquidity confirmed", { shares, hash, token: tokenSymbol });
    } else {
      console.log("DRY_RUN: add-liquidity would submit", { ...addParams, token: tokenSymbol });
    }
  }

  // 5. Read the LP token balance after the add-liquidity transaction confirms.
  if (lpTokenId) {
    console.log("LP shares", await new TokenClient(config(lpTokenId)).balance(sourceAddress));
  } else {
    console.log("Set LP_TOKEN_CONTRACT_ID to read LP shares.");
  }

  // 6. Remove liquidity — opt-in via REMOVE_LIQUIDITY=1.
  if (process.env.REMOVE_LIQUIDITY === "1" && lpTokenId) {
    const shares = await new TokenClient(config(lpTokenId)).balance(sourceAddress);
    if (shares > 0n) {
      const removeDeadline = BigInt(Math.floor(Date.now() / 1000) + deadlineSeconds);
      const removeParams = {
        provider: sourceAddress,
        shares,
        minAmountA: 1n,
        minAmountB: 1n,
        deadline: removeDeadline,
      };
      if (signer) {
        const { returnValue: [amountA, amountB], hash } = await pool.removeLiquidity(removeParams, signer);
        console.log("Remove-liquidity confirmed", { amountA, amountB, shares, hash });
      } else {
        console.log("DRY_RUN: remove-liquidity would submit", removeParams);
      }
    } else {
      console.log("No LP shares to remove.");
    }
  }

  if (factory) console.log("Known pools", await factory.allPools());
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`AMM contract call failed: ${message}. See docs/error-codes.md for recovery guidance.`);
  process.exitCode = 1;
});
