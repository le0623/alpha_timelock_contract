# Alpha Vault — Timelock Contract

An ink! smart contract deployed on Bittensor (Subtensor) that manages coldkey ownership of alpha (subnet stake) for a configurable lock period.

Alpha itself never moves. Only the coldkey owner changes via `transfer_stake`. The hotkey and staking position on the subnet remain unchanged throughout.

---

## When the Contract Is Used

The contract is invoked **every time alpha ownership moves to a user** — which happens in three scenarios:

| Scenario | Alpha recipient |
|----------|----------------|
| Fill a sell order (buyer fills) | Buyer receives alpha |
| Fill a buy order (seller fills) | Buyer receives alpha |
| Close a sell order (no fill) | Seller gets alpha back |

In all cases, instead of transferring alpha ownership directly to the user, the backend first transfers it to the contract coldkey, records a lock, and the user claims ownership after the lock period expires.

---

## How It Works

```
1. Alpha ownership moves to the contract
   transfer_stake(escrow coldkey → contract coldkey, same hotkey, netuid)
   → contract now owns the alpha on that hotkey

2. Backend records the lock
   contract.lock(recipient_coldkey, hotkey, netuid, amount, lock_blocks)
   → records who receives ownership and when

3. After lock_blocks elapse, recipient claims ownership
   contract.release(deposit_id)
   → transfer_stake(contract coldkey → recipient coldkey, same hotkey, netuid)
   → recipient now owns the alpha; hotkey and subnet unchanged
```

---

## Messages

| Message | Caller | Description |
|---------|--------|-------------|
| `lock(recipient, hotkey, netuid, amount, lock_blocks)` | Owner | Record a new lock after `transfer_stake(escrow → contract)` |
| `release(deposit_id)` | Recipient only | Transfer ownership to recipient after lock expires |
| `emergency_release(deposit_id)` | Owner only | Force ownership transfer before expiry; still goes to the recorded recipient |
| `set_min_lock_blocks(blocks)` | Owner only | Update minimum lock duration |
| `transfer_ownership(new_owner)` | Owner only | Hand contract ownership to a new account |
| `is_locked(deposit_id)` | Anyone | Returns `true` if lock is still active |
| `blocks_remaining(deposit_id)` | Anyone | Blocks until lock expires (0 if expired) |
| `get_lock(deposit_id)` | Anyone | Full lock record |

---

## Chain Extension

Uses the Subtensor chain extension (extension `0`, function `6` — `transfer_stake`).  
Reference: [Subtensor WASM Contracts](https://github.com/opentensor/subtensor/blob/main/docs/wasm-contracts.md)

---

## Build

Requires [cargo-contract](https://github.com/paritytech/cargo-contract) and ink! `< 6.0`.

```sh
cargo contract build
```

Artifacts are output to `target/ink/`.

## Test

```sh
cargo test
```

---

## Notes

- `amount` is in rao units (`1 alpha = 1_000_000_000 rao`).
- The contract owner is the account that deploys the contract (the backend operator).
- `emergency_release` cannot redirect alpha — it always goes to the recipient recorded at lock time.
- One contract handles all trades; each trade gets a unique `deposit_id`.
