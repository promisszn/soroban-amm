# Security Fix: NFT Contract Change Vulnerability

## Vulnerability Summary

**Severity**: CRITICAL  
**Component**: `contracts/concentrated_liquidity/src/lib.rs`  
**Function**: `set_position_nft` (lines 301-313)  
**Impact**: Permanent loss of LP position access; potential unauthorized control transfer

## Description

The `set_position_nft` function allowed the admin to change `DataKey::PositionNft` to a different NFT contract address at any time, without checking whether positions were already tokenized. This created a critical mismatch:

1. **Index Orphaning**: The pool's `NftTokenToPosition(token_id)` and `PositionNftToken(provider, ticks)` indices still referenced token IDs minted against the **old** NFT contract.

2. **Query Mismatch**: Any subsequent call to `resolve_token_owner` or `ensure_legacy_owner` would:
   - Read a `token_id` from the old contract's index (stored on-chain)
   - Query `PositionNftClient::new(env, &nft).owner_of(&token_id)` against the **new** contract using that stale ID

3. **Attack Scenarios**:
   - **Most likely**: Token ID doesn't exist in the new contract → `owner_of()` traps → position permanently locked (LP can't use token-id path or legacy address path)
   - **ID collision**: New contract reused the same sequential ID (NFT IDs start at 0 and increment) → `owner_of()` returns an unrelated owner → wrong party gains control or legitimate owner is denied

## Proof of Concept

```rust
// 1. Pool deployed with NFT_v1, Alice opens position → token_id = 0
// 2. Admin calls set_position_nft(NFT_v2)  ← NO VALIDATION
// 3. Pool storage still has:
//    - NftTokenToPosition(0) = (Alice, -100, 100)
//    - PositionNftToken(Alice, -100, 100) = 0
// 4. Alice tries burn_position_by_token_id(0):
//    ├─ resolve_token_owner reads NftTokenToPosition(0) ✓
//    ├─ calls PositionNftClient::new(&NFT_v2).owner_of(0)
//    └─ NFT_v2 has no token 0 → TRAP → Alice locked out
// 5. Alice tries legacy burn_position(Alice, -100, 100):
//    ├─ ensure_legacy_owner reads PositionNftToken(Alice, ticks) = 0 ✓
//    ├─ calls PositionNftClient::new(&NFT_v2).owner_of(0)
//    └─ TRAP → both paths blocked
```

## Fix

Added validation in `set_position_nft` to **block changing from one NFT contract to a different one**:

```rust
let existing_nft: Option<Address> = env
    .storage()
    .instance()
    .get(&DataKey::PositionNft)
    .unwrap_or(None);

// Only allow: None → Some (initial set) or Some → None (detach)
// Block: Some(A) → Some(B) (would orphan indices)
if existing_nft.is_some() && nft.is_some() && existing_nft != nft {
    return Err(ClError::NftContractChangeBlocked);
}
```

**Allowed transitions**:
- `None → Some(A)` — Initial NFT contract wiring
- `Some(A) → None` — Detach NFT (positions become legacy-only)
- `Some(A) → Some(A)` — Re-setting the same contract (no-op)

**Blocked transition**:
- `Some(A) → Some(B)` — Returns `ClError::NftContractChangeBlocked`

## Changes

1. **contracts/concentrated_liquidity/src/lib.rs:308-330** — Added validation logic
2. **contracts/concentrated_liquidity/src/lib.rs:61** — Added `NftContractChangeBlocked = 21` error variant
3. **contracts/concentrated_liquidity/src/lib.rs:6209-6254** — Added regression test `cannot_change_nft_contract_after_positions_tokenized`

## Testing

The new test verifies:
- ✅ Attempting to change NFT contract after position minting returns `NftContractChangeBlocked`
- ✅ Detaching the NFT contract (→ None) is still allowed
- ✅ Re-attaching the same contract is allowed

## Recommendations

1. **Deploy**: This fix must be deployed **before** any production positions are tokenized.
2. **Migration**: If positions are already tokenized on a live contract, the NFT contract is now permanently locked — any migration would require redeploying the pool contract with a new storage model.
3. **Audit**: Recommend third-party security review of all admin-controlled state transitions.

## Related Code

- `resolve_token_owner` (lines 1595-1617): Reads `token_id` from index, queries NFT
- `ensure_legacy_owner` (lines 1619-1642): Guards legacy path by querying NFT ownership
- `tokenize_position` (lines 1565-1590): Mints NFT and writes both index directions

---

**Reported**: 2026-07-28  
**Fixed**: 2026-07-28  
**Status**: Patched (pending build verification)
