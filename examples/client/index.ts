import "dotenv/config";

import {
  BASE_FEE,
  Contract,
  Keypair,
  Networks,
  Transaction,
  TransactionBuilder,
  nativeToScVal,
  rpc as StellarRpc,
  scValToNative,
  xdr,
} from "@stellar/stellar-sdk";

const rpcUrl = process.env.STELLAR_RPC_URL ?? "https://soroban-testnet.stellar.org";
const networkPassphrase = process.env.STELLAR_NETWORK_PASSPHRASE ?? Networks.TESTNET;

const ammContractId = requiredEnv("AMM_CONTRACT_ID");
const sourceSecret = requiredEnv("SOURCE_SECRET");
const tokenInContractId = requiredEnv("TOKEN_IN_CONTRACT_ID");

const swapAmountIn = parseI128(process.env.SWAP_AMOUNT_IN ?? "100000");
const swapMinOut = parseI128(process.env.SWAP_MIN_OUT ?? "0");

const sourceKeypair = Keypair.fromSecret(sourceSecret);
const traderAddress = sourceKeypair.publicKey();
const lpProviderAddress = process.env.LP_PROVIDER_ADDRESS ?? traderAddress;

const server = new StellarRpc.Server(rpcUrl);
const amm = new Contract(ammContractId);

async function main(): Promise<void> {
  console.log(`Connected to ${rpcUrl}`);
  console.log(`AMM contract: ${ammContractId}`);

  const poolInfo = await simulateContractCall("get_info");
  console.log("Pool info:");
  console.log(formatScVal(poolInfo));

  const quotedAmountOut = await simulateContractCall(
    "get_amount_out",
    addressArg(tokenInContractId),
    i128Arg(swapAmountIn),
  );
  console.log("Quote:");
  console.log(formatScVal({ tokenInContractId, amountIn: swapAmountIn, amountOut: quotedAmountOut }));

  const lpShares = await simulateContractCall("shares_of", addressArg(lpProviderAddress));
  console.log("LP share balance:");
  console.log(formatScVal({ provider: lpProviderAddress, shares: lpShares }));

  const swapResult = await submitContractCall(
    "swap",
    addressArg(traderAddress),
    addressArg(tokenInContractId),
    i128Arg(swapAmountIn),
    i128Arg(swapMinOut),
  );
  console.log("Swap submitted:");
  console.log(formatScVal({ trader: traderAddress, amountOut: swapResult }));
}

async function simulateContractCall(method: string, ...args: xdr.ScVal[]): Promise<unknown> {
  const transaction = await buildTransaction(amm.call(method, ...args));
  const simulation = await server.simulateTransaction(transaction);

  if (StellarRpc.Api.isSimulationError(simulation)) {
    throw new Error(`Simulation failed: ${simulation.error}`);
  }

  const returnValue = simulation.result?.retval;
  if (!returnValue) {
    throw new Error(`Simulation for ${method} did not return a value`);
  }

  return scValToNative(returnValue);
}

async function submitContractCall(method: string, ...args: xdr.ScVal[]): Promise<unknown> {
  const transaction = await buildTransaction(amm.call(method, ...args));
  const preparedTransaction = await server.prepareTransaction(transaction);
  preparedTransaction.sign(sourceKeypair);

  const sendResponse = await server.sendTransaction(preparedTransaction);
  if (sendResponse.status !== "PENDING") {
    throw new Error(`Transaction submission failed: ${JSON.stringify(sendResponse)}`);
  }

  const finalResponse = await pollTransaction(sendResponse.hash);
  if (finalResponse.status !== "SUCCESS") {
    throw new Error(`Transaction failed with status ${finalResponse.status}`);
  }

  const returnValue = (finalResponse as { returnValue?: xdr.ScVal }).returnValue;
  if (!returnValue) {
    throw new Error(`Transaction for ${method} did not return a value`);
  }

  return scValToNative(returnValue);
}

async function buildTransaction(operation: xdr.Operation): Promise<Transaction> {
  const sourceAccount = await server.getAccount(sourceKeypair.publicKey());

  return new TransactionBuilder(sourceAccount, {
    fee: BASE_FEE,
    networkPassphrase,
  })
    .addOperation(operation)
    .setTimeout(30)
    .build();
}

async function pollTransaction(hash: string): Promise<StellarRpc.Api.GetTransactionResponse> {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const response = await server.getTransaction(hash);

    if (response.status !== "NOT_FOUND") {
      return response;
    }

    await sleep(1000);
  }

  throw new Error(`Timed out waiting for transaction ${hash}`);
}

function addressArg(address: string): xdr.ScVal {
  return nativeToScVal(address, { type: "address" });
}

function i128Arg(value: bigint): xdr.ScVal {
  return nativeToScVal(value, { type: "i128" });
}

function parseI128(value: string): bigint {
  if (!/^-?\d+$/.test(value)) {
    throw new Error(`Expected an integer i128 value, got "${value}"`);
  }

  return BigInt(value);
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }

  return value;
}

function formatScVal(value: unknown): string {
  return JSON.stringify(normalizeForJson(value), null, 2);
}

function normalizeForJson(value: unknown): unknown {
  if (typeof value === "bigint") {
    return value.toString();
  }

  if (value instanceof Map) {
    return Object.fromEntries(
      Array.from(value.entries()).map(([key, entryValue]) => [
        String(key),
        normalizeForJson(entryValue),
      ]),
    );
  }

  if (Array.isArray(value)) {
    return value.map((entry) => normalizeForJson(entry));
  }

  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, entryValue]) => [key, normalizeForJson(entryValue)]),
    );
  }

  return value;
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, milliseconds);
  });
}

main().catch((error: unknown) => {
  console.error(error);
  process.exitCode = 1;
});
