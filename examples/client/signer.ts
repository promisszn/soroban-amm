/**
 * Signer adapters for the Soroban AMM client example.
 *
 * Two shapes are provided:
 *
 * - `KeypairSigner` — signs with a secret key read from the `SIGNING_SECRET_KEY`
 *   environment variable. Suitable for scripts, CI, and testnet use. Keep the
 *   secret key in a `.env` file that is never committed.
 *
 * - `SignerFunc` — the raw function type the SDK accepts. Any async function
 *   that takes the base64-encoded unsigned XDR and returns the signed XDR
 *   satisfies this interface, so HSM adapters, browser wallet bridges, and
 *   remote signers all plug in without modification.
 *
 * Usage:
 * ```ts
 * import { makeKeypairSigner } from "./signer.js";
 * const signer = makeKeypairSigner(); // reads SIGNING_SECRET_KEY
 * const amountOut = await pool.swap(params, signer);
 * ```
 */

import { Keypair, TransactionBuilder } from "@stellar/stellar-sdk";
import type { Signer } from "@soroban-amm/sdk";

export type { Signer as SignerFunc };

/**
 * Return a `Signer` backed by a Stellar `Keypair` loaded from the
 * `SIGNING_SECRET_KEY` environment variable.
 *
 * The variable must be a 56-character Stellar secret key (`S...`).
 * The function throws at construction time if the variable is absent or
 * malformed so the error surfaces before any RPC call is attempted.
 *
 * @param networkPassphrase — forwarded to `TransactionBuilder.fromXDR` so the
 *   signed envelope targets the correct network.
 */
export function makeKeypairSigner(networkPassphrase: string): Signer {
  const raw = process.env.SIGNING_SECRET_KEY;
  if (!raw) {
    throw new Error(
      "SIGNING_SECRET_KEY is required for write operations; set it to a funded testnet secret key (S...)"
    );
  }
  let keypair: Keypair;
  try {
    keypair = Keypair.fromSecret(raw);
  } catch {
    throw new Error(
      "SIGNING_SECRET_KEY is not a valid Stellar secret key; it must start with 'S' and be 56 characters"
    );
  }

  return async (unsignedXdr: string): Promise<string> => {
    const tx = TransactionBuilder.fromXDR(unsignedXdr, networkPassphrase);
    tx.sign(keypair);
    return tx.toXDR();
  };
}
