# Alpha Vault — Timelock Contract

An ink! smart contract deployed on Bittensor (Subtensor) that manages coldkey ownership of alpha (subnet stake) for a configurable lock period.

Alpha itself never moves. Only the coldkey owner changes via `transfer_stake`. The hotkey and staking position on the subnet remain unchanged throughout.

---

## How It Works

```
1. Fill order executes
   transfer_stake(escrow coldkey → contract coldkey, same hotkey, netuid)
   → contract now owns the alpha on that hotkey

2. Backend records the lock
   contract.lock(filler_coldkey, hotkey, netuid, amount, lock_blocks)

3. After lock_blocks elapse, filler claims ownership
   contract.release(deposit_id)
   → transfer_stake(contract coldkey → filler coldkey, same hotkey, netuid)
   → filler now owns the alpha; hotkey and subnet unchanged
```

---

## Messages

| Message | Caller | Description |
|---------|--------|-------------|
| `lock(filler, hotkey, netuid, amount, lock_blocks)` | Owner | Record a new lock after `transfer_stake(escrow → contract)` |
| `release(deposit_id)` | Filler only | Transfer ownership to filler after lock expires |
| `emergency_release(deposit_id)` | Owner only | Force ownership transfer before expiry; still goes to filler |
| `set_min_lock_blocks(blocks)` | Owner only | Update minimum lock duration |
| `transfer_ownership(new_owner)` | Owner only | Hand ownership to a new account |
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
- The contract owner is the account that deploys the contract (typically the backend operator).
- `emergency_release` cannot redirect alpha — it always goes to the filler recorded at lock time.
