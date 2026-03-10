#![cfg_attr(not(feature = "std"), no_std, no_main)]

//! # Alpha Vault — Single-Use Timelock Contract
//!
//! One contract instance = one trade. Deploy a fresh instance per trade.
//!
//! Flow:
//! 1. Escrow: `transfer_stake(escrow → contract coldkey, hotkey, netuid)`
//!    — alpha ownership moves to the contract.
//! 2. Escrow: `contract.lock(buyer_coldkey, hotkey, netuid, amount, lock_blocks)`
//!    — callable exactly once; records the buyer and seals the contract.
//!    — after this call no further state-changing calls are accepted except `release`.
//! 3. Buyer: `contract.release()` after lock expires
//!    — ownership transfers from contract coldkey → buyer coldkey.

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
    SameAutoStakeHotkeyAlreadySet,
    InsufficientBalance,
    AmountTooLow,
    InsufficientLiquidity,
    SameNetuid,
    ProxyTooMany,
    ProxyDuplicate,
    ProxyNoSelfProxy,
    ProxyNotFound,
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
            11 => Err(SubtensorError::SameAutoStakeHotkeyAlreadySet),
            12 => Err(SubtensorError::InsufficientBalance),
            13 => Err(SubtensorError::AmountTooLow),
            14 => Err(SubtensorError::InsufficientLiquidity),
            15 => Err(SubtensorError::SameNetuid),
            16 => Err(SubtensorError::ProxyTooMany),
            17 => Err(SubtensorError::ProxyDuplicate),
            18 => Err(SubtensorError::ProxyNoSelfProxy),
            19 => Err(SubtensorError::ProxyNotFound),
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

    /// Transfers coldkey ownership of alpha to `destination_coldkey`.
    /// The hotkey and subnet position are unchanged.
    ///
    /// Parameter order matches the Subtensor chain extension (function 6):
    /// `(destination_coldkey, hotkey, origin_netuid, destination_netuid, alpha_amount)`
    #[ink(function = 6, handle_status = true)]
    fn transfer_stake(
        destination_coldkey: ink::primitives::AccountId,
        hotkey: ink::primitives::AccountId,
        origin_netuid: u16,
        destination_netuid: u16,
        alpha_amount: u64,
    ) -> core::result::Result<(), SubtensorError>;
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[ink::contract(env = crate::SubtensorEnvironment)]
mod alpha_vault {
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

    // ── Storage ───────────────────────────────────────────────────────────────

    /// State machine:
    ///   Unlocked  →  (escrow calls lock())  →  Locked
    ///   Locked    →  (buyer calls release() after expiry)  →  Released
    #[derive(Debug, Clone, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[cfg_attr(feature = "std", derive(ink::storage::traits::StorageLayout))]
    #[allow(clippy::cast_possible_truncation)]
    pub enum VaultState {
        /// No lock registered yet; only `lock()` is accepted.
        Unlocked,
        /// Lock registered; only `release()` is accepted.
        Locked {
            buyer_coldkey: AccountId,
            hotkey: AccountId,
            netuid: u16,
            amount: u64,
            lock_until: BlockNumber,
        },
        /// Alpha has been released to the buyer; contract is inert.
        Released,
    }

    #[ink(storage)]
    pub struct AlphaVault {
        escrow: AccountId,
        state: VaultState,
    }

    // ── Events ────────────────────────────────────────────────────────────────

    #[ink(event)]
    pub struct Locked {
        #[ink(topic)]
        pub buyer_coldkey: AccountId,
        pub hotkey: AccountId,
        pub netuid: u16,
        pub amount: u64,
        pub lock_until: BlockNumber,
    }

    #[ink(event)]
    pub struct Released {
        #[ink(topic)]
        pub buyer_coldkey: AccountId,
        pub amount: u64,
        pub released_at: BlockNumber,
    }

    // ── Errors ────────────────────────────────────────────────────────────────

    #[derive(Debug, PartialEq, Eq)]
    #[ink::scale_derive(Encode, Decode, TypeInfo)]
    #[allow(clippy::cast_possible_truncation)]
    pub enum Error {
        /// `lock()` was called by someone other than the escrow (deployer).
        NotEscrow,
        /// `lock()` was called but a lock already exists.
        AlreadyLocked,
        /// `release()` was called but no lock has been registered yet.
        NotLocked,
        /// `release()` was called but the alpha was already released.
        AlreadyReleased,
        /// Caller is not the registered buyer.
        NotBuyer,
        /// Lock period has not expired yet.
        LockNotExpired,
        /// Amount must be greater than zero.
        ZeroAmount,
        /// Lock duration must be greater than zero.
        ZeroLockBlocks,
        /// The chain extension call to `transfer_stake` failed.
        TransferStakeFailed(SubtensorError),
    }

    pub type Result<T> = core::result::Result<T, Error>;

    // ── Messages ──────────────────────────────────────────────────────────────

    impl AlphaVault {
        /// Deploy a fresh vault. The deployer (caller) is recorded as the escrow —
        /// the only account permitted to call `lock()`.
        #[ink(constructor)]
        pub fn new() -> Self {
            Self {
                escrow: Self::env().caller(),
                state: VaultState::Unlocked,
            }
        }

        /// Register the lock. Callable exactly once, only by the escrow (deployer).
        ///
        /// Must be called after `transfer_stake(escrow → contract coldkey)` so the
        /// contract already owns the alpha. After this call the only accepted
        /// state-changing message is `release()` by the buyer.
        #[ink(message)]
        pub fn lock(
            &mut self,
            buyer_coldkey: AccountId,
            hotkey: AccountId,
            netuid: u16,
            amount: u64,
            lock_blocks: BlockNumber,
        ) -> Result<()> {
            if self.env().caller() != self.escrow {
                return Err(Error::NotEscrow);
            }

            match self.state {
                VaultState::Unlocked => {}
                VaultState::Locked { .. } | VaultState::Released => {
                    return Err(Error::AlreadyLocked);
                }
            }

            if amount == 0 {
                return Err(Error::ZeroAmount);
            }
            if lock_blocks == 0 {
                return Err(Error::ZeroLockBlocks);
            }

            let current_block = self.env().block_number();
            let lock_until = current_block.saturating_add(lock_blocks);

            self.state = VaultState::Locked {
                buyer_coldkey,
                hotkey,
                netuid,
                amount,
                lock_until,
            };

            self.env().emit_event(Locked {
                buyer_coldkey,
                hotkey,
                netuid,
                amount,
                lock_until,
            });

            Ok(())
        }

        /// Release alpha to the buyer. Callable **only by the buyer**, after the lock expires.
        ///
        /// Transfers coldkey ownership from the contract to the buyer.
        /// The hotkey and subnet position remain unchanged.
        #[ink(message)]
        pub fn release(&mut self) -> Result<()> {
            let (buyer_coldkey, hotkey, netuid, amount, lock_until) = match self.state {
                VaultState::Unlocked => return Err(Error::NotLocked),
                VaultState::Released => return Err(Error::AlreadyReleased),
                VaultState::Locked { buyer_coldkey, hotkey, netuid, amount, lock_until } => {
                    (buyer_coldkey, hotkey, netuid, amount, lock_until)
                }
            };

            if self.env().caller() != buyer_coldkey {
                return Err(Error::NotBuyer);
            }

            let current_block = self.env().block_number();
            if current_block < lock_until {
                return Err(Error::LockNotExpired);
            }

            // Perform the external transfer before updating state so that a
            // failed transfer leaves the vault in Locked (not Released).
            self.env()
                .extension()
                .transfer_stake(buyer_coldkey, hotkey, netuid, netuid, amount)
                .map_err(Error::TransferStakeFailed)?;

            self.state = VaultState::Released;

            self.env().emit_event(Released {
                buyer_coldkey,
                amount,
                released_at: current_block,
            });

            Ok(())
        }

        // ── Read-only queries ─────────────────────────────────────────────────

        /// Returns the escrow (deployer) account that is allowed to call `lock()`.
        #[ink(message)]
        pub fn get_escrow(&self) -> AccountId {
            self.escrow
        }

        /// Returns the current vault state.
        #[ink(message)]
        pub fn get_state(&self) -> VaultState {
            self.state.clone()
        }

        /// Returns true if a lock is registered and the lock period has not expired.
        #[ink(message)]
        pub fn is_locked(&self) -> bool {
            match self.state {
                VaultState::Locked { lock_until, .. } => {
                    self.env().block_number() < lock_until
                }
                _ => false,
            }
        }

        /// Returns the number of blocks remaining until the lock expires (0 if not locked or expired).
        #[ink(message)]
        pub fn blocks_remaining(&self) -> BlockNumber {
            match self.state {
                VaultState::Locked { lock_until, .. } => {
                    let current = self.env().block_number();
                    if current < lock_until { lock_until.saturating_sub(current) } else { 0 }
                }
                _ => 0,
            }
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

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
        fn constructor_starts_unlocked() {
            let a = accounts();
            set_caller(a.alice);
            let vault = AlphaVault::new();
            assert_eq!(vault.get_escrow(), a.alice);
            assert_eq!(vault.get_state(), VaultState::Unlocked);
            assert!(!vault.is_locked());
            assert_eq!(vault.blocks_remaining(), 0);
        }

        #[ink::test]
        fn lock_records_correctly() {
            let a = accounts();
            set_caller(a.alice); // alice = escrow
            let mut vault = AlphaVault::new();

            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("lock failed");

            assert!(vault.is_locked());
            assert_eq!(vault.blocks_remaining(), 10);

            match vault.get_state() {
                VaultState::Locked { buyer_coldkey, hotkey, netuid, amount, .. } => {
                    assert_eq!(buyer_coldkey, a.bob);
                    assert_eq!(hotkey, a.charlie);
                    assert_eq!(netuid, 1);
                    assert_eq!(amount, 1_000_000_000);
                }
                _ => panic!("expected Locked state"),
            }
        }

        #[ink::test]
        fn lock_can_only_be_called_once() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new();

            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("first lock failed");
            // second call must fail
            assert_eq!(
                vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10),
                Err(Error::AlreadyLocked)
            );
        }

        #[ink::test]
        fn non_escrow_cannot_lock() {
            let a = accounts();
            set_caller(a.alice); // alice deploys → she is the escrow
            let mut vault = AlphaVault::new();
            set_caller(a.bob); // bob tries to lock
            assert_eq!(
                vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10),
                Err(Error::NotEscrow)
            );
        }

        #[ink::test]
        fn zero_amount_rejected() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new();
            assert_eq!(vault.lock(a.bob, a.charlie, 1, 0, 10), Err(Error::ZeroAmount));
        }

        #[ink::test]
        fn zero_lock_blocks_rejected() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new();
            assert_eq!(vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 0), Err(Error::ZeroLockBlocks));
        }

        #[ink::test]
        fn release_without_lock_rejected() {
            let a = accounts();
            set_caller(a.bob);
            let mut vault = AlphaVault::new();
            assert_eq!(vault.release(), Err(Error::NotLocked));
        }

        #[ink::test]
        fn release_before_expiry_rejected() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new();
            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("lock failed");
            set_caller(a.bob);
            assert_eq!(vault.release(), Err(Error::LockNotExpired));
        }

        #[ink::test]
        fn non_buyer_cannot_release() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new();
            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("lock failed");
            advance_blocks(11);
            set_caller(a.charlie); // hotkey, not buyer
            assert_eq!(vault.release(), Err(Error::NotBuyer));
        }

        #[ink::test]
        fn blocks_remaining_decreases() {
            let a = accounts();
            set_caller(a.alice);
            let mut vault = AlphaVault::new();
            vault.lock(a.bob, a.charlie, 1, 1_000_000_000, 10).expect("lock failed");

            assert_eq!(vault.blocks_remaining(), 10);
            advance_blocks(5);
            assert_eq!(vault.blocks_remaining(), 5);
            advance_blocks(5);
            assert_eq!(vault.blocks_remaining(), 0);
            assert!(!vault.is_locked());
        }
    }
}

pub use alpha_vault::SubtensorEnvironment;
