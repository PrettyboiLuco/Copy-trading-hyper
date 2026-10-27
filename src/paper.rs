use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::{Fill, Mids, Side};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PaperPosition {
    /// Signed size: positive = long, negative = short.
    pub size: f64,
    pub entry_px: f64,
}

/// Simulated perp account. `cash` is the USDC collateral and already includes
/// realized PnL and paid fees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperAccount {
    pub cash: f64,
    pub starting_balance: f64,
    pub positions: HashMap<String, PaperPosition>,
    /// Taker fee as a fraction of notional (Hyperliquid base tier: 0.045%).
    pub fee_rate: f64,
    /// Slippage applied against us, in basis points of the mid price.
    pub slippage_bps: f64,
    pub realized_pnl: f64,
    pub fees_paid: f64,
    pub trade_count: u64,
}

impl PaperAccount {
    pub fn new(balance: f64, fee_rate: f64, slippage_bps: f64) -> Self {
        Self {
            cash: balance,
            starting_balance: balance,
            positions: HashMap::new(),
            fee_rate,
            slippage_bps,
            realized_pnl: 0.0,
            fees_paid: 0.0,
            trade_count: 0,
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&data).with_context(|| format!("invalid state in {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn size(&self, coin: &str) -> f64 {
        self.positions.get(coin).map_or(0.0, |p| p.size)
    }

    pub fn unrealized_pnl(&self, mids: &Mids) -> f64 {
        self.positions
            .iter()
            .map(|(coin, p)| {
                let px = mids.get(coin).copied().unwrap_or(p.entry_px);
                p.size * (px - p.entry_px)
            })
            .fold(0.0, |acc, x| acc + x)
    }

    pub fn equity(&self, mids: &Mids) -> f64 {
        self.cash + self.unrealized_pnl(mids)
    }

    pub fn notional(&self, mids: &Mids) -> f64 {
        self.positions
            .iter()
            .map(|(coin, p)| {
                let px = mids.get(coin).copied().unwrap_or(p.entry_px);
                (p.size * px).abs()
            })
            .fold(0.0, |acc, x| acc + x)
    }

    /// Execute a market order of signed `delta` at `mid` (plus slippage).
    pub fn execute(&mut self, coin: &str, delta: f64, mid: f64) -> Fill {
        let side = Side::of(delta);
        let slip = self.slippage_bps / 10_000.0;
        let price = match side {
            Side::Buy => mid * (1.0 + slip),
            Side::Sell => mid * (1.0 - slip),
        };
        let fee = delta.abs() * price * self.fee_rate;

        let pos = self.positions.entry(coin.to_string()).or_default();
        let old = pos.size;
        let new = old + delta;
        let mut realized = 0.0;

        if old == 0.0 || old.signum() == delta.signum() {
            // Opening or increasing: volume-weighted entry.
            pos.entry_px = (old.abs() * pos.entry_px + delta.abs() * price) / new.abs();
        } else {
            // Reducing, closing or flipping.
            let closed = delta.abs().min(old.abs());
            realized = closed * (price - pos.entry_px) * old.signum();
            if new != 0.0 && new.signum() != old.signum() {
                pos.entry_px = price;
            }
        }
        pos.size = new;
        if is_dust(pos.size) {
            self.positions.remove(coin);
        }

        self.cash += realized - fee;
        self.realized_pnl += realized;
        self.fees_paid += fee;
        self.trade_count += 1;

        Fill {
            coin: coin.to_string(),
            side,
            size: delta.abs(),
            price,
            fee,
            realized_pnl: realized,
        }
    }
}

fn is_dust(size: f64) -> bool {
    size.abs() < 1e-9
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::mids;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    #[test]
    fn open_increase_and_close_long() {
        let mut acc = PaperAccount::new(1_000.0, 0.0, 0.0);
        acc.execute("BTC", 0.01, 60_000.0);
        acc.execute("BTC", 0.01, 62_000.0);
        approx(acc.positions["BTC"].entry_px, 61_000.0);
        approx(acc.equity(&mids(&[("BTC", 63_000.0)])), 1_040.0);

        let fill = acc.execute("BTC", -0.02, 63_000.0);
        approx(fill.realized_pnl, 40.0);
        assert!(acc.positions.is_empty());
        approx(acc.cash, 1_040.0);
    }

    #[test]
    fn short_profit_and_flip() {
        let mut acc = PaperAccount::new(1_000.0, 0.0, 0.0);
        acc.execute("ETH", -1.0, 3_000.0);
        // Buy 3: closes the 1 short (+100) and opens 2 long at 2900.
        let fill = acc.execute("ETH", 3.0, 2_900.0);
        approx(fill.realized_pnl, 100.0);
        approx(acc.size("ETH"), 2.0);
        approx(acc.positions["ETH"].entry_px, 2_900.0);
    }

    #[test]
    fn fees_and_slippage_are_charged() {
        let mut acc = PaperAccount::new(1_000.0, 0.001, 10.0);
        let fill = acc.execute("BTC", 0.01, 50_000.0);
        approx(fill.price, 50_050.0);
        approx(fill.fee, 0.5005);
        approx(acc.cash, 1_000.0 - 0.5005);
    }
}
