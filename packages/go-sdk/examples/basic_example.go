// Command basic_example walks through a full read-then-write session against a
// deployed AMM pool: quote a swap, inspect the pool, then submit the swap with
// an explicit slippage bound and deadline.
//
// It needs a Soroban RPC endpoint and a pool contract id. The write half also
// needs a Signer; this example wires in a placeholder that returns an error, so
// running it as-is exercises the read path and stops cleanly at the write.
package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"math/big"
	"os"
	"time"

	gosdk "github.com/promisszn/soroban-amm/packages/go-sdk"
)

func main() {
	rpcURL := envOr("SOROBAN_RPC_URL", "https://soroban-testnet.stellar.org")
	poolID := os.Getenv("AMM_POOL_ID")
	tokenIn := os.Getenv("AMM_TOKEN_IN")
	trader := os.Getenv("AMM_TRADER")

	if poolID == "" || tokenIn == "" {
		log.Fatal("set AMM_POOL_ID and AMM_TOKEN_IN to a deployed pool and one of its tokens")
	}

	// Read-only clients need no signer. Supply one only for the write half.
	client, err := gosdk.NewClient(gosdk.Config{
		RPCURL:            rpcURL,
		NetworkPassphrase: gosdk.NetworkTestnet,
		Timeout:           20 * time.Second,
		Signer:            newExampleSigner(trader),
	})
	if err != nil {
		log.Fatalf("client: %v", err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	// 1. Read the pool's current state. This runs through simulation, so it
	//    costs nothing and needs no signature.
	info, err := client.GetInfo(ctx, poolID)
	if err != nil {
		log.Fatalf("get_info: %v", err)
	}
	fmt.Printf("pool %s/%s reserves %s/%s fee %s bps\n",
		info.TokenA, info.TokenB, info.ReserveA, info.ReserveB, info.FeeBps)

	// 2. Quote the swap before committing to it. The quote carries the fee and
	//    the price impact, not just the output amount.
	amountIn := big.NewInt(10_000_000) // 1.0 unit at 7 decimals
	quote, err := client.SimulateSwap(ctx, poolID, tokenIn, amountIn)
	if err != nil {
		log.Fatalf("simulate_swap: %v", err)
	}
	fmt.Printf("quote: out=%s fee=%s impact=%s bps\n",
		quote.AmountOut, quote.FeeAmount, quote.PriceImpactBps)

	// 3. Derive a slippage bound from the quote. 50 bps of tolerance here; pick
	//    a value that suits the pool's depth and your latency to the network.
	minOut := applySlippageBps(quote.AmountOut, 50)

	// 4. Submit. Both the deadline and the slippage bound are required — the
	//    SDK will not invent either, because a silent default is exactly how a
	//    swap gets sandwiched.
	res, err := client.Swap(ctx, gosdk.SwapParams{
		PoolID:   poolID,
		Trader:   trader,
		TokenIn:  tokenIn,
		AmountIn: amountIn,
		MinOut:   minOut,
		Deadline: uint64(time.Now().Add(2 * time.Minute).Unix()),
	})
	switch {
	case errors.Is(err, gosdk.ErrSlippageExceeded):
		// The pool moved between the quote and inclusion. Re-quote and retry
		// with a fresh bound rather than widening this one blindly.
		log.Fatalf("swap: price moved past min_out=%s", minOut)
	case errors.Is(err, gosdk.ErrDeadlineExceeded):
		log.Fatal("swap: deadline passed before inclusion; use a longer window")
	case errors.Is(err, gosdk.ErrPaused):
		log.Fatal("swap: pool is paused")
	case errors.Is(err, gosdk.ErrNoSigner):
		log.Fatal("swap: no signer configured; supply Config.Signer to submit writes")
	case err != nil:
		log.Fatalf("swap: %v", err)
	}

	fmt.Printf("swap submitted: %s (%s)\n", res.Hash, res.Status)
}

// applySlippageBps returns amount reduced by the given number of basis points.
func applySlippageBps(amount *big.Int, bps int64) *big.Int {
	out := new(big.Int).Mul(amount, big.NewInt(10_000-bps))
	return out.Div(out, big.NewInt(10_000))
}

// newExampleSigner returns a placeholder signer. Replace the SignEnvelope body
// with a real keypair, an HSM call, or a request to a remote signing service.
func newExampleSigner(address string) gosdk.Signer {
	if !gosdk.IsAccountAddress(address) {
		return nil
	}
	return gosdk.SignerFunc{
		Addr: address,
		Sign: func(ctx context.Context, envelopeXDR, networkPassphrase string) (string, error) {
			return "", errors.New("example signer: plug in a real signing backend")
		},
	}
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}
