import { keypairSigner } from "@soroban-amm/sdk";
import type { Signer } from "@soroban-amm/sdk";

/** Load the testnet signing key without logging or exposing the secret. */
export function makeKeypairSigner(): Signer {
  const raw = process.env.SIGNING_SECRET_KEY;
  if (!raw) {
    throw new Error(
      "SIGNING_SECRET_KEY is required for write operations; set it to a funded testnet secret key (S...)"
    );
  }
  try {
    return keypairSigner(raw);
  } catch {
    throw new Error(
      "SIGNING_SECRET_KEY is not a valid Stellar secret key; it must start with 'S' and be 56 characters"
    );
  }
}
