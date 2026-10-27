use anyhow::Result;

use crate::types::{AccountSnapshot, Mids};

/// Where the bot gets the target wallet state and prices from.
/// Implemented by the live Hyperliquid client and by the offline mock.
pub trait MarketSource {
    fn target_snapshot(&mut self, wallet: &str) -> Result<AccountSnapshot>;
    fn mids(&mut self) -> Result<Mids>;
}
