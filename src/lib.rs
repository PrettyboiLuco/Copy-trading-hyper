//! Paper-trading copy bot for Hyperliquid.
//!
//! The bot polls the perp positions of a target wallet, scales them to the
//! size of a simulated (paper) account and "executes" the difference against
//! current mid prices. No real orders are ever sent and no private key is used.

pub mod engine;
pub mod hyperliquid;
pub mod journal;
pub mod mock;
pub mod paper;
pub mod source;
pub mod types;

/// Wallet followed by default in test runs.
pub const DEFAULT_TARGET_WALLET: &str = "0x0c1f6941FDcDe9488DfD36cA16FeD6B3bc49672B";
