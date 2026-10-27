use std::collections::{BTreeSet, HashMap};

use anyhow::Result;

use crate::paper::PaperAccount;
use crate::source::MarketSource;
use crate::types::{AccountSnapshot, Fill, Mids};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sizing {
    /// Scale target sizes by `my_equity / target_equity * multiplier`.
    Proportional { multiplier: f64 },
    /// Copy target sizes times a fixed ratio (e.g. 0.01 = 1%).
    Fixed { ratio: f64 },
}

#[derive(Debug, Clone)]
pub struct CopyConfig {
    pub wallet: String,
    pub sizing: Sizing,
    /// Total notional is capped at `max_leverage * equity`.
    pub max_leverage: f64,
    /// Orders below this notional are skipped (Hyperliquid minimum is $10).
    pub min_order_usd: f64,
    /// Optional cap on the notional of a single coin.
    pub max_position_usd: Option<f64>,
    /// Ignore adjustments smaller than this fraction of the desired size.
    pub rebalance_threshold: f64,
    /// Do not copy positions the target already holds when the bot starts.
    pub skip_existing: bool,
}

impl CopyConfig {
    pub fn new(wallet: impl Into<String>) -> Self {
        Self {
            wallet: wallet.into(),
            sizing: Sizing::Proportional { multiplier: 1.0 },
            max_leverage: 5.0,
            min_order_usd: 10.0,
            max_position_usd: None,
            rebalance_threshold: 0.05,
            skip_existing: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderIntent {
    pub coin: String,
    pub delta: f64,
    pub mid: f64,
}

pub struct CopyEngine {
    pub config: CopyConfig,
    pub account: PaperAccount,
    /// Coins held by the target at startup (with their sign) that are ignored
    /// until the target closes or flips them. Only used with `skip_existing`.
    ignored: HashMap<String, f64>,
    initialized: bool,
}

impl CopyEngine {
    pub fn new(config: CopyConfig, account: PaperAccount) -> Self {
        Self {
            config,
            account,
            ignored: HashMap::new(),
            initialized: false,
        }
    }

    pub fn ignored_coins(&self) -> Vec<String> {
        let mut coins: Vec<_> = self.ignored.keys().cloned().collect();
        coins.sort();
        coins
    }

    fn update_ignored(&mut self, target: &AccountSnapshot) {
        if !self.initialized {
            self.initialized = true;
            if self.config.skip_existing {
                self.ignored = target
                    .positions
                    .iter()
                    .filter(|(coin, _)| self.account.size(coin) == 0.0)
                    .map(|(coin, size)| (coin.clone(), size.signum()))
                    .collect();
            }
        }
        self.ignored.retain(|coin, sign| {
            let now = target.positions.get(coin).copied().unwrap_or(0.0);
            now != 0.0 && now.signum() == *sign
        });
    }

    /// Target position per coin for the paper account.
    pub fn desired_positions(&self, target: &AccountSnapshot, mids: &Mids) -> HashMap<String, f64> {
        let equity = self.account.equity(mids);
        let scale = match self.config.sizing {
            Sizing::Proportional { multiplier } if target.account_value > 0.0 => {
                equity / target.account_value * multiplier
            }
            Sizing::Proportional { .. } => 0.0,
            Sizing::Fixed { ratio } => ratio,
        };

        let mut desired: HashMap<String, f64> = HashMap::new();
        for (coin, size) in &target.positions {
            if self.ignored.contains_key(coin) {
                continue;
            }
            let Some(&px) = mids.get(coin) else { continue };
            let mut want = size * scale;
            if let Some(cap) = self.config.max_position_usd
                && (want * px).abs() > cap
            {
                want = cap / px * want.signum();
            }
            desired.insert(coin.clone(), want);
        }

        let notional: f64 = desired
            .iter()
            .map(|(coin, size)| (size * mids[coin]).abs())
            .sum();
        let max_notional = self.config.max_leverage * equity.max(0.0);
        if notional > max_notional && notional > 0.0 {
            let factor = max_notional / notional;
            desired.values_mut().for_each(|s| *s *= factor);
        }
        desired
    }

    /// Orders needed to move the paper account towards the target.
    pub fn plan(&mut self, target: &AccountSnapshot, mids: &Mids) -> Vec<OrderIntent> {
        self.update_ignored(target);
        let desired = self.desired_positions(target, mids);

        let coins: BTreeSet<String> = desired
            .keys()
            .chain(self.account.positions.keys())
            .cloned()
            .collect();

        let mut orders = Vec::new();
        for coin in coins {
            let Some(&mid) = mids.get(&coin) else { continue };
            let current = self.account.size(&coin);
            let want = desired.get(&coin).copied().unwrap_or(0.0);

            // Target closed the coin: always close fully, regardless of size.
            if want == 0.0 && !self.ignored.contains_key(&coin) {
                if current != 0.0 {
                    orders.push(OrderIntent { coin, delta: -current, mid });
                }
                continue;
            }

            let delta = want - current;
            if (delta * mid).abs() < self.config.min_order_usd {
                continue;
            }
            let same_direction = current != 0.0 && current.signum() == want.signum();
            if same_direction && delta.abs() < self.config.rebalance_threshold * want.abs() {
                continue;
            }
            orders.push(OrderIntent { coin, delta, mid });
        }
        orders
    }

    pub fn execute(&mut self, orders: &[OrderIntent]) -> Vec<Fill> {
        orders
            .iter()
            .map(|o| self.account.execute(&o.coin, o.delta, o.mid))
            .collect()
    }

    /// One polling cycle: fetch target + prices, plan and execute.
    pub fn tick(&mut self, source: &mut dyn MarketSource) -> Result<TickReport> {
        let target = source.target_snapshot(&self.config.wallet)?;
        let mids = source.mids()?;
        let orders = self.plan(&target, &mids);
        let fills = self.execute(&orders);
        Ok(TickReport {
            target,
            equity: self.account.equity(&mids),
            notional: self.account.notional(&mids),
            fills,
            mids,
        })
    }
}

#[derive(Debug, Clone)]
pub struct TickReport {
    pub target: AccountSnapshot,
    pub mids: Mids,
    pub fills: Vec<Fill>,
    pub equity: f64,
    pub notional: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::mids;

    fn engine(balance: f64) -> CopyEngine {
        let mut cfg = CopyConfig::new("0xtest");
        cfg.max_leverage = 100.0;
        CopyEngine::new(cfg, PaperAccount::new(balance, 0.0, 0.0))
    }

    #[test]
    fn proportional_sizing() {
        let mut e = engine(1_000.0);
        let target = AccountSnapshot::new(100_000.0, &[("BTC", 1.0), ("ETH", -10.0)]);
        let px = mids(&[("BTC", 60_000.0), ("ETH", 3_000.0)]);
        let orders = e.plan(&target, &px);
        assert_eq!(orders.len(), 2);
        let btc = orders.iter().find(|o| o.coin == "BTC").unwrap();
        let eth = orders.iter().find(|o| o.coin == "ETH").unwrap();
        assert!((btc.delta - 0.01).abs() < 1e-12);
        assert!((eth.delta + 0.1).abs() < 1e-12);
    }

    #[test]
    fn fixed_sizing() {
        let mut e = engine(1_000.0);
        e.config.sizing = Sizing::Fixed { ratio: 0.002 };
        let target = AccountSnapshot::new(100_000.0, &[("BTC", 5.0)]);
        let orders = e.plan(&target, &mids(&[("BTC", 60_000.0)]));
        assert!((orders[0].delta - 0.01).abs() < 1e-12);
    }

    #[test]
    fn leverage_cap_scales_down() {
        let mut e = engine(1_000.0);
        e.config.max_leverage = 2.0;
        // Target runs 6x -> we would get 6_000 notional, cap is 2_000.
        let target = AccountSnapshot::new(100_000.0, &[("BTC", 10.0)]);
        let px = mids(&[("BTC", 60_000.0)]);
        let orders = e.plan(&target, &px);
        assert!((orders[0].delta * 60_000.0 - 2_000.0).abs() < 1e-6);
    }

    #[test]
    fn max_position_cap() {
        let mut e = engine(1_000.0);
        e.config.max_position_usd = Some(300.0);
        let target = AccountSnapshot::new(100_000.0, &[("BTC", 1.0)]);
        let orders = e.plan(&target, &mids(&[("BTC", 60_000.0)]));
        assert!((orders[0].delta * 60_000.0 - 300.0).abs() < 1e-6);
    }

    #[test]
    fn skips_orders_below_minimum() {
        let mut e = engine(100.0);
        // Desired: 0.0001 BTC = $6 < $10 minimum.
        let target = AccountSnapshot::new(100_000.0, &[("BTC", 0.1)]);
        assert!(e.plan(&target, &mids(&[("BTC", 60_000.0)])).is_empty());
    }

    #[test]
    fn closes_when_target_closes_even_if_tiny() {
        let mut e = engine(1_000.0);
        e.account.execute("BTC", 0.0001, 60_000.0);
        let target = AccountSnapshot::new(100_000.0, &[]);
        let orders = e.plan(&target, &mids(&[("BTC", 60_000.0)]));
        assert_eq!(orders.len(), 1);
        assert!((orders[0].delta + 0.0001).abs() < 1e-12);
    }

    #[test]
    fn ignores_small_rebalances() {
        let mut e = engine(1_000.0);
        e.config.min_order_usd = 0.0;
        e.account.execute("BTC", 0.01, 60_000.0);
        // Target equity moved slightly -> desired 0.0102, delta 2% < 5% threshold.
        let target = AccountSnapshot::new(98_000.0, &[("BTC", 1.0)]);
        assert!(e.plan(&target, &mids(&[("BTC", 60_000.0)])).is_empty());
    }

    #[test]
    fn skip_existing_until_target_closes() {
        let mut e = engine(1_000.0);
        e.config.skip_existing = true;
        let px = mids(&[("BTC", 60_000.0), ("ETH", 3_000.0)]);

        let start = AccountSnapshot::new(100_000.0, &[("BTC", 1.0)]);
        assert!(e.plan(&start, &px).is_empty());
        assert_eq!(e.ignored_coins(), vec!["BTC".to_string()]);

        // New ETH trade is copied, BTC still ignored even when increased.
        let next = AccountSnapshot::new(100_000.0, &[("BTC", 2.0), ("ETH", 5.0)]);
        let orders = e.plan(&next, &px);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].coin, "ETH");

        // Target closes BTC, then reopens it -> now copied.
        e.plan(&AccountSnapshot::new(100_000.0, &[("ETH", 5.0)]), &px);
        assert!(e.ignored_coins().is_empty());
        let orders = e.plan(&AccountSnapshot::new(100_000.0, &[("BTC", 1.0), ("ETH", 5.0)]), &px);
        assert!(orders.iter().any(|o| o.coin == "BTC"));
    }

    #[test]
    fn skips_coins_without_price() {
        let mut e = engine(1_000.0);
        let target = AccountSnapshot::new(100_000.0, &[("XYZ", 100.0)]);
        assert!(e.plan(&target, &mids(&[("BTC", 60_000.0)])).is_empty());
    }
}
