#![cfg_attr(not(feature = "std"), no_std, no_main)]

//! # Alpha Vault — Timelock Contract
//!
//! Manages coldkey ownership of alpha (subnet stake) for a lock period.
//! The hotkey and the staking position on the subnet remain unchanged throughout.
//!
//! Flow:
//! 1. Backend: `transfer_stake(escrow → contract coldkey, hotkey, netuid)`
//!    — ownership moves to the contract; alpha stays on the same hotkey.
//! 2. Backend: `contract.lock(filler, hotkey, netuid, amount, lock_blocks)`
//!    — records who receives ownership after the lock expires.
//! 3. Filler: `contract.release(deposit_id)` after lock expires
//!    — ownership transfers from contract coldkey → filler coldkey (hotkey, subnet).

// ── Chain Extension ──────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
#[ink::scale_derive(Encode, Decode, TypeInfo)]
#[allow(clippy::cast_possible_truncation)]
pub enum SubtensorError {
    RuntimeError,
    NotEnoughBalanceToStake,
    NonAssociatedColdKey,
    BalanceWithdrawalError,
    NotRegistered,
    NotEnoughStakeToWithdraw,
    TxRateLimitExceeded,
    SlippageTooHigh,
    SubnetNotExists,
    HotKeyNotRegisteredInSubNet,
    InsufficientBalance,
    AmountTooLow,
    InsufficientLiquidity,
    Unknown(u32),
}

impl ink::env::chain_extension::FromStatusCode for SubtensorError {
    fn from_status_code(status_code: u32) -> core::result::Result<(), Self> {
        match status_code {
            0  => Ok(()),
            1  => Err(SubtensorError::RuntimeError),
            2  => Err(SubtensorError::NotEnoughBalanceToStake),
            3  => Err(SubtensorError::NonAssociatedColdKey),
            4  => Err(SubtensorError::BalanceWithdrawalError),
            5  => Err(SubtensorError::NotRegistered),
            6  => Err(SubtensorError::NotEnoughStakeToWithdraw),
            7  => Err(SubtensorError::TxRateLimitExceeded),
            8  => Err(SubtensorError::SlippageTooHigh),
            9  => Err(SubtensorError::SubnetNotExists),
            10 => Err(SubtensorError::HotKeyNotRegisteredInSubNet),
            12 => Err(SubtensorError::InsufficientBalance),
            13 => Err(SubtensorError::AmountTooLow),
            14 => Err(SubtensorError::InsufficientLiquidity),
            n  => Err(SubtensorError::Unknown(n)),
        }
    }
}

impl From<ink::scale::Error> for SubtensorError {
    fn from(_: ink::scale::Error) -> Self {
        SubtensorError::RuntimeError
    }
}

/// Subtensor chain extension (extension = 0).
#[ink::chain_extension(extension = 0)]
pub trait SubtensorExtension {
    type ErrorCode = SubtensorError;

    /// Transfer stake ownership between coldkeys.
    /// Transfers coldkey ownership of alpha to `destination_coldkey`.
    /// The hotkey and subnet position are unchanged.
    #[ink(function = 6, handle_status = true)]
    fn transfer_stake(
        hotkey: ink::primitives::AccountId,
        destination_coldkey: ink::primitives::AccountId,
        origin_netuid: u16,
        destination_netuid: u16,
        alpha_amount: u64,
    ) -> core::result::Result<(), SubtensorError>;
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[ink::contract(env = crate::SubtensorEnvironment)]
mod alpha_vault {
    use ink::storage::Mapping;
    use crate::SubtensorError;

    #[derive(Debug, Clone, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    pub enum SubtensorEnvironment {}

    impl ink::env::Environment for SubtensorEnvironment {
        const MAX_EVENT_TOPICS: usize = 3;
        type AccountId    = <ink::env::DefaultEnvironment as ink::env::Environment>::AccountId;
        type Balance      = <ink::env::DefaultEnvironment as ink::env::Environment>::Balance;
        type Hash         = <ink::env::DefaultEnvironment as ink::env::Environment>::Hash;
        type BlockNumber  = <ink::env::DefaultEnvironment as ink::env::Environment>::BlockNumber;
        type Timestamp    = <ink::env::DefaultEnvironment as ink::env::Environment>::Timestamp;
        type ChainExtension = crate::SubtensorExtension;
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    pub struct LockRecord {
        pub filler_coldkey: AccountId,  // receives coldkey ownership on release
        pub hotkey: AccountId,          // unchanged throughout; alpha stays on this hotkey
        pub netuid: u16,
        pub amount: u64,                // alpha in rao (1 alpha = 1e9 rao)
        pub lock_start: BlockNumber,
        pub lock_until: BlockNumber,
        pub released: bool,
    }

    #[ink(storage)]
    pub struct AlphaVault {
        owner: AccountId,               // backend operator
        next_id: u64,
        locks: Mapping<u64, LockRecord>,
        min_lock_blocks: BlockNumber,
        total_locks: u64,
        total_released: u64,
    }

    #[ink(event)]
    pub struct Locked {
        #[ink(topic)]
        pub deposit_id: u64,
        #[ink(topic)]
        pub filler_coldkey: AccountId,
        pub hotkey: AccountId,
        pub netuid: u16,
        pub amount: u64,
        pub lock_until: BlockNumber,
    }

    #[ink(event)]
    pub struct Released {
        #[ink(topic)]
        pub deposit_id: u64,
        #[ink(topic)]
        pub filler_coldkey: AccountId,
        pub amount: u64,
        pub released_at: BlockNumber,
    }

    #[ink(event)]
    pub struct OwnerChanged {
        pub old_owner: AccountId,
        pub new_owner: AccountId,
    }

    #[derive(Debug, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[allow(clippy::cast_possible_truncation)]
    pub enum Error {
        NotOwner,
        NotFiller,
        LockNotFound,
        AlreadyReleased,
        LockNotExpired,
        InvalidLockPeriod,
        ZeroAmount,
        TransferStakeFailed(SubtensorError),
    }

    pub type Result<T> = core::result::Result<T, Error>;

    impl AlphaVault {
        /// Deploy. `min_lock_blocks` = minimum allowed lock duration (0 = no minimum).
        #[ink(constructor)]
        pub fn new(min_lock_blocks: BlockNumber) -> Self {
            Self {
                owner: Self::env().caller(),
                next_id: 0,
                locks: Mapping::default(),
                min_lock_blocks,
                total_locks: 0,
                total_released: 0,
            }
        }

        /// Record a new lock. Owner only.
        /// Call after `transfer_stake(escrow → contract coldkey)` has already been executed,
        /// so the contract coldkey already owns the alpha on `hotkey`.
        /// `amount` is in rao units (1 alpha = 1_000_000_000 rao).
        #[ink(message)]
        pub fn lock(
            &mut self,
            filler_coldkey: AccountId,
            hotkey: AccountId,
            netuid: u16,
            amount: u64,
            lock_blocks: BlockNumber,
        ) -> Result<u64> {
            if self.env().caller() != self.owner {
                return Err(Error::NotOwner);
            }
            if amount == 0 {
                return Err(Error::ZeroAmount);
            }
            if lock_blocks == 0 || lock_blocks < self.min_lock_blocks {
                return Err(Error::InvalidLockPeriod);
            }

            let current_block = self.env().block_number();
            let lock_until = current_block.saturating_add(lock_blocks);
            let deposit_id = self.next_id;
            self.next_id = self.next_id.saturating_add(1);

            self.locks.insert(deposit_id, &LockRecord {
                filler_coldkey,
                hotkey,
                netuid,
                amount,
                lock_start: current_block,
                lock_until,
                released: false,
            });
            self.total_locks = self.total_locks.saturating_add(1);

            self.env().emit_event(Locked {
                deposit_id,
                filler_coldkey,
                hotkey,
                netuid,
                amount,
                lock_until,
            });

            Ok(deposit_id)
        }

        /// Transfer alpha ownership to the filler. Filler only, after lock expires.
        /// Alpha does not move — only coldkey ownership transfers from the contract to the filler.
        /// The hotkey and subnet position remain unchanged.
        #[ink(message)]
        pub fn release(&mut self, deposit_id: u64) -> Result<()> {
            let mut record = self.locks.get(deposit_id).ok_or(Error::LockNotFound)?;

            if record.released {
                return Err(Error::AlreadyReleased);
            }
            if self.env().caller() != record.filler_coldkey {
                return Err(Error::NotFiller);
            }
            let current_block = self.env().block_number();
            if current_block < record.lock_until {
                return Err(Error::LockNotExpired);
            }

            record.released = true;
            self.locks.insert(deposit_id, &record);
            self.total_released = self.total_released.saturating_add(1);

            self.env()
                .extension()
                .transfer_stake(record.hotkey, record.filler_coldkey, record.netuid, record.netuid, record.amount)
                .map_err(Error::TransferStakeFailed)?;

            self.env().emit_event(Released {
                deposit_id,
                filler_coldkey: record.filler_coldkey,
                amount: record.amount,
                released_at: current_block,
            });

            Ok(())
        }

        /// Force ownership transfer before expiry. Owner only.
        /// Ownership still goes to the filler — owner cannot redirect it elsewhere.
        #[ink(message)]
        pub fn emergency_release(&mut self, deposit_id: u64) -> Result<()> {
            if self.env().caller() != self.owner {
                return Err(Error::NotOwner);
            }

            let mut record = self.locks.get(deposit_id).ok_or(Error::LockNotFound)?;

            if record.released {
                return Err(Error::AlreadyReleased);
            }

            record.released = true;
            self.locks.insert(deposit_id, &record);
            self.total_released = self.total_released.saturating_add(1);

            self.env()
                .extension()
                .transfer_stake(record.hotkey, record.filler_coldkey, record.netuid, record.netuid, record.amount)
                .map_err(Error::TransferStakeFailed)?;

            let current_block = self.env().block_number();
            self.env().emit_event(Released {
                deposit_id,
                filler_coldkey: record.filler_coldkey,
                amount: record.amount,
                released_at: current_block,
            });

            Ok(())
        }

        /// Set minimum lock period. Owner only.
        #[ink(message)]
        pub fn set_min_lock_blocks(&mut self, blocks: BlockNumber) -> Result<()> {
            if self.env().caller() != self.owner {
                return Err(Error::NotOwner);
            }
            self.min_lock_blocks = blocks;
            Ok(())
        }

        /// Transfer ownership. Owner only.
        #[ink(message)]
        pub fn transfer_ownership(&mut self, new_owner: AccountId) -> Result<()> {
            if self.env().caller() != self.owner {
                return Err(Error::NotOwner);
            }
            let old = self.owner;
            self.owner = new_owner;
            self.env().emit_event(OwnerChanged { old_owner: old, new_owner });
            Ok(())
        }

        #[ink(message)]
        pub fn is_locked(&self, deposit_id: u64) -> bool {
            match self.locks.get(deposit_id) {
                Some(r) => !r.released && self.env().block_number() < r.lock_until,
                None => false,
            }
        }

        #[ink(message)]
        pub fn blocks_remaining(&self, deposit_id: u64) -> BlockNumber {
            match self.locks.get(deposit_id) {
                Some(r) if !r.released => {
                    let current = self.env().block_number();
                    if current < r.lock_until { r.lock_until.saturating_sub(current) } else { 0 }
                }
                _ => 0,
            }
        }

        #[ink(message)]
        pub fn get_lock(&self, deposit_id: u64) -> Option<LockRecord> {
            self.locks.get(deposit_id)
        }

        #[ink(message)]
        pub fn get_next_id(&self) -> u64 { self.next_id }

        #[ink(message)]
        pub fn get_owner(&self) -> AccountId { self.owner }

        #[ink(message)]
        pub fn get_min_lock_blocks(&self) -> BlockNumber { self.min_lock_blocks }

        #[ink(message)]
        pub fn get_total_locks(&self) -> u64 { self.total_locks }

        #[ink(message)]
        pub fn get_total_released(&self) -> u64 { self.total_released }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn accounts() -> ink::env::test::DefaultAccounts<ink::env::DefaultEnvironment> {
            ink::env::test::default_accounts::<ink::env::DefaultEnvironment>()
        }

        fn set_caller(caller: AccountId) {
            ink::env::test::set_caller::<ink::env::DefaultEnvironment>(caller);
        }

        fn advance_blocks(n: u32) {
            for _ in 0..n {
                ink::env::test::advance_block::<ink::env::DefaultEnvironment>();
            }
        }

        #[ink::test]
        fn constructor_works() {
            let a = accounts();
            set_caller(a.alice);
            let vault = AlphaVault::new(10);
            assert_eq!(vault.get_owner(), a.alice);
            assert_eq!(vault.get_min_lock_blocks(), 10);
            assert_eq!(vault.get_next_id(), 0);
        }

        #[ink::test]
        fn lock_records_correctly() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);

            let id = vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("lock failed");

            assert_eq!(id, 0);
            assert!(vault.is_locked(id));
            assert_eq!(vault.blocks_remaining(id), 10);
            assert_eq!(vault.get_total_locks(), 1);

            let rec = vault.get_lock(id).expect("record missing");
            assert_eq!(rec.filler_coldkey, a.bob);
            assert_eq!(rec.hotkey, a.charlie);
            assert_eq!(rec.netuid, 1);
            assert_eq!(rec.amount, 1_000_000_000);
            assert!(!rec.released);
        }

        #[ink::test]
        fn non_owner_cannot_lock() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);
            set_caller(a.bob);
            assert_eq!(vault.lock(a.charlie, a.django, 1, 1_000_000_000, 10), Err(Error::NotOwner));
        }

        #[ink::test]
        fn zero_amount_rejected() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);
            assert_eq!(vault.lock(a.bob, a.charlie, 1, 0, 10), Err(Error::ZeroAmount));
        }

        #[ink::test]
        fn lock_period_too_short() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(10);
            assert_eq!(vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 5), Err(Error::InvalidLockPeriod));
        }

        #[ink::test]
        fn release_before_expiry_rejected() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);
            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("lock failed");
            set_caller(a.bob);
            assert_eq!(vault.release(0), Err(Error::LockNotExpired));
        }

        #[ink::test]
        fn non_filler_cannot_release() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);
            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("lock failed");
            advance_blocks(11);
            set_caller(a.charlie); // hotkey, not filler
            assert_eq!(vault.release(0), Err(Error::NotFiller));
        }

        #[ink::test]
        fn lock_not_found() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);
            set_caller(a.bob);
            assert_eq!(vault.release(99), Err(Error::LockNotFound));
        }

        #[ink::test]
        fn blocks_remaining_decreases() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);
            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("lock failed");

            assert_eq!(vault.blocks_remaining(0), 10);
            advance_blocks(5);
            assert_eq!(vault.blocks_remaining(0), 5);
            advance_blocks(5);
            assert_eq!(vault.blocks_remaining(0), 0);
            assert!(!vault.is_locked(0));
        }

        #[ink::test]
        fn transfer_ownership_works() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);
            vault.transfer_ownership(a.bob).expect("transfer failed");
            assert_eq!(vault.get_owner(), a.bob);
            assert_eq!(vault.set_min_lock_blocks(20), Err(Error::NotOwner));
        }

        #[ink::test]
        fn non_owner_cannot_emergency_release() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new(5);
            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 100).expect("lock failed");
            set_caller(a.bob);
            assert_eq!(vault.emergency_release(0), Err(Error::NotOwner));
        }
    }
}

pub use alpha_vault::SubtensorEnvironment;
