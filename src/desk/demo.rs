//! Synthetic wallet for running the desk without network access: random-walk
//! prices, a trader that opens/adds/reduces/closes/flips positions, TP/SL
//! orders that trigger, and resting limit entries that fill.

use std::collections::HashMap;

use anyhow::Result;
use chrono::Utc;

use super::{DeskSnapshot, DeskSource};
use crate::hyperliquid::{OpenOrder, PositionInfo};
use crate::types::{AccountSnapshot, Mids, Side};
use crate::ws::TradeEvent;

const LEVERAGE: f64 = 10.0;

struct Coin {
    name: &'static str,
    price: f64,
    vol: f64,
}

#[derive(Clone, Copy)]
struct Pos {
    size: f64,
    entry: f64,
}

pub struct DemoSource {
    seed: u64,
    coins: Vec<Coin>,
    positions: HashMap<&'static str, Pos>,
    orders: Vec<OpenOrder>,
    balance: f64,
    next_oid: u64,
    tick: u64,
}

impl DemoSource {
    pub fn new(seed: u64) -> Self {
        let coin = |name, price, vol| Coin { name, price, vol };
        Self {
            seed: seed | 1,
            coins: vec![
                coin("BTC", 76_200.0, 0.0018),
                coin("ETH", 3_050.0, 0.0024),
                coin("SOL", 168.0, 0.0032),
                coin("HYPE", 41.5, 0.0045),
                coin("DOGE", 0.21, 0.004),
            ],
            positions: HashMap::new(),
            orders: Vec::new(),
            balance: 1_850_000.0,
            next_oid: 9_000_000,
            tick: 0,
        }
    }

    fn rand(&mut self) -> f64 {
        // xorshift64*
        self.seed ^= self.seed >> 12;
        self.seed ^= self.seed << 25;
        self.seed ^= self.seed >> 27;
        (self.seed.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        (0..4).map(|_| self.rand()).sum::<f64>() - 2.0
    }

    fn price(&self, name: &str) -> f64 {
        self.coins
            .iter()
            .find(|c| c.name == name)
            .map_or(0.0, |c| c.price)
    }

    fn mids(&self) -> Mids {
        self.coins
            .iter()
            .map(|c| (c.name.to_string(), c.price))
            .collect()
    }

    fn equity(&self) -> f64 {
        self.balance
            + self
                .positions
                .iter()
                .map(|(c, p)| p.size * (self.price(c) - p.entry))
                .sum::<f64>()
    }

    fn oid(&mut self) -> u64 {
        self.next_oid += 1 + (self.rand() * 40.0) as u64;
        self.next_oid
    }

    /// Execute a market order of signed `delta` for the synthetic trader.
    fn trade(&mut self, coin: &'static str, delta: f64) -> TradeEvent {
        let px = self.price(coin);
        let pos = self.positions.get(coin).copied().unwrap_or(Pos {
            size: 0.0,
            entry: px,
        });
        let new = pos.size + delta;
        let mut closed_pnl = 0.0;
        let entry = if pos.size == 0.0 || pos.size.signum() == delta.signum() {
            (pos.size.abs() * pos.entry + delta.abs() * px) / new.abs()
        } else {
            closed_pnl = delta.abs().min(pos.size.abs()) * (px - pos.entry) * pos.size.signum();
            if new != 0.0 && new.signum() != pos.size.signum() {
                px
            } else {
                pos.entry
            }
        };
        let fee = delta.abs() * px * 0.00025;
        self.balance += closed_pnl - fee;
        if new.abs() < 1e-12 {
            self.positions.remove(coin);
        } else {
            self.positions.insert(coin, Pos { size: new, entry });
        }
        self.reset_tpsl(coin);

        let side = Side::of(delta);
        let dir = match (pos.size == 0.0, new == 0.0 || new.abs() < 1e-12) {
            (true, _) => format!("Open {}", if delta > 0.0 { "Long" } else { "Short" }),
            (_, true) => format!("Close {}", if pos.size > 0.0 { "Long" } else { "Short" }),
            _ if new.signum() != pos.size.signum() => {
                if new > 0.0 {
                    "Short > Long".into()
                } else {
                    "Long > Short".into()
                }
            }
            _ if new.abs() > pos.size.abs() => {
                format!("Open {}", if new > 0.0 { "Long" } else { "Short" })
            }
            _ => format!("Close {}", if pos.size > 0.0 { "Long" } else { "Short" }),
        };
        TradeEvent {
            coin: coin.to_string(),
            side,
            size: delta.abs(),
            avg_px: px,
            start_position: pos.size,
            closed_pnl,
            fee,
            time_ms: Utc::now().timestamp_millis() as u64,
            oid: self.oid(),
            dir,
        }
    }

    fn order(
        &mut self,
        coin: &str,
        side: Side,
        size: f64,
        px: f64,
        kind: &str,
        trigger: bool,
    ) -> OpenOrder {
        OpenOrder {
            coin: coin.to_string(),
            side: if side == Side::Buy { "B" } else { "A" }.into(),
            limit_px: fmt_px(px),
            sz: format!("{size}"),
            oid: self.oid(),
            timestamp: Utc::now().timestamp_millis() as u64,
            order_type: kind.into(),
            reduce_only: trigger,
            is_trigger: trigger,
            trigger_px: if trigger { fmt_px(px) } else { String::new() },
            is_position_tpsl: trigger,
        }
    }

    /// Replace the TP/SL pair of `coin` to cover the full position.
    fn reset_tpsl(&mut self, coin: &str) {
        self.orders
            .retain(|o| !(o.coin == coin && o.is_position_tpsl));
        let Some(pos) = self.positions.get(coin).copied() else {
            return;
        };
        let long = pos.size > 0.0;
        let exit = if long { Side::Sell } else { Side::Buy };
        let (tp, sl) = if long {
            (pos.entry * 1.045, pos.entry * 0.975)
        } else {
            (pos.entry * 0.955, pos.entry * 1.025)
        };
        let tp = self.order(coin, exit, pos.size.abs(), tp, "Take Profit Market", true);
        let sl = self.order(coin, exit, pos.size.abs(), sl, "Stop Market", true);
        self.orders.push(tp);
        self.orders.push(sl);
    }

    fn size_for(&self, coin: &str, share: f64) -> f64 {
        let notional = self.equity() * LEVERAGE * share;
        let px = self.price(coin);
        let raw = notional / px;
        let decimals = if px > 10_000.0 {
            3
        } else if px > 100.0 {
            2
        } else if px > 1.0 {
            1
        } else {
            0
        };
        let f = 10f64.powi(decimals);
        ((raw * f).round() / f).max(1.0 / f)
    }

    fn step(&mut self) -> Vec<TradeEvent> {
        self.tick += 1;
        let mut trades = Vec::new();

        let shocks: Vec<f64> = (0..self.coins.len()).map(|_| self.gauss()).collect();
        let market = self.gauss() * 0.6;
        for (coin, shock) in self.coins.iter_mut().zip(shocks) {
            coin.price *= 1.0 + coin.vol * (shock + market);
        }

        // Triggered TP/SL and filled resting limits.
        let hits: Vec<OpenOrder> = self
            .orders
            .iter()
            .filter(|o| {
                let px = self.price(&o.coin);
                let level: f64 = if o.is_trigger {
                    &o.trigger_px
                } else {
                    &o.limit_px
                }
                .parse()
                .unwrap_or(0.0);
                let buy = o.side == "B";
                match (o.is_trigger, o.order_type.as_str()) {
                    (true, "Take Profit Market") => {
                        if buy {
                            px <= level
                        } else {
                            px >= level
                        }
                    }
                    (true, _) => {
                        if buy {
                            px >= level
                        } else {
                            px <= level
                        }
                    }
                    (false, _) => {
                        if buy {
                            px <= level
                        } else {
                            px >= level
                        }
                    }
                }
            })
            .cloned()
            .collect();
        for hit in hits {
            if !self.orders.iter().any(|o| o.oid == hit.oid) {
                continue; // removed by an earlier hit (TP and SL of one position)
            }
            self.orders.retain(|o| o.oid != hit.oid);
            let Some(coin) = self.coins.iter().map(|c| c.name).find(|n| *n == hit.coin) else {
                continue;
            };
            let size: f64 = hit.sz.parse().unwrap_or(0.0);
            let delta = if hit.side == "B" { size } else { -size };
            let delta = if hit.reduce_only {
                let held = self.positions.get(coin).map_or(0.0, |p| p.size);
                if held == 0.0 || held.signum() == delta.signum() {
                    continue;
                }
                -held
            } else {
                delta
            };
            trades.push(self.trade(coin, delta));
        }

        // The trader acts: quickly at the start, then every few ticks.
        let act = self.tick == 2 || self.tick == 4 || self.rand() < 0.16;
        if act {
            let held: Vec<&'static str> = self.positions.keys().copied().collect();
            let free: Vec<&'static str> = self
                .coins
                .iter()
                .map(|c| c.name)
                .filter(|n| !self.positions.contains_key(n))
                .collect();
            let r = self.rand();
            if (held.len() < 2 || (held.len() < 4 && r < 0.35)) && !free.is_empty() {
                let coin = free[(self.rand() * free.len() as f64) as usize % free.len()];
                let long = self.rand() < 0.6;
                let share = 0.04 + self.rand() * 0.08;
                let size = self.size_for(coin, share);
                trades.push(self.trade(coin, if long { size } else { -size }));
            } else if !held.is_empty() {
                let coin = held[(self.rand() * held.len() as f64) as usize % held.len()];
                let pos = self.positions[coin];
                let r = self.rand();
                let delta = if r < 0.38 {
                    pos.size * (0.25 + self.rand() * 0.4) // add
                } else if r < 0.72 {
                    -pos.size * (0.3 + self.rand() * 0.3) // reduce
                } else if r < 0.92 {
                    -pos.size // close
                } else {
                    -pos.size * 2.0 // flip
                };
                let decimals = if self.price(coin) > 10_000.0 { 3 } else { 2 };
                let f = 10f64.powi(decimals);
                let delta = if r < 0.72 {
                    (delta * f).round() / f
                } else {
                    delta
                };
                if delta != 0.0 {
                    trades.push(self.trade(coin, delta));
                }
            }
        }

        // Occasionally park a limit entry below/above the market.
        let resting = self.orders.iter().filter(|o| !o.is_trigger).count();
        if resting < 2 && self.rand() < 0.12 {
            let free: Vec<&'static str> = self
                .coins
                .iter()
                .map(|c| c.name)
                .filter(|n| {
                    !self.positions.contains_key(n) && !self.orders.iter().any(|o| o.coin == *n)
                })
                .collect();
            if !free.is_empty() {
                let coin = free[(self.rand() * free.len() as f64) as usize % free.len()];
                let long = self.rand() < 0.55;
                let px = self.price(coin) * if long { 0.993 } else { 1.007 };
                let size = self.size_for(coin, 0.05);
                let side = if long { Side::Buy } else { Side::Sell };
                let order = self.order(coin, side, size, px, "Limit", false);
                self.orders.push(order);
            }
        }
        trades
    }
}

fn fmt_px(px: f64) -> String {
    let decimals = if px > 1_000.0 {
        1
    } else if px > 10.0 {
        3
    } else {
        5
    };
    format!("{px:.decimals$}")
}

impl DeskSource for DemoSource {
    fn poll(&mut self, _wallet: &str) -> Result<DeskSnapshot> {
        let trades = self.step();
        let account_value = self.equity();
        let mut positions: Vec<PositionInfo> = self
            .positions
            .iter()
            .map(|(coin, p)| {
                let px = self.price(coin);
                let liq = p.entry * (1.0 - p.size.signum() * 0.9 / LEVERAGE);
                PositionInfo {
                    coin: coin.to_string(),
                    size: p.size,
                    entry_px: p.entry,
                    position_value: (p.size * px).abs(),
                    unrealized_pnl: p.size * (px - p.entry),
                    liquidation_px: Some(liq),
                    leverage: Some(format!("{LEVERAGE}x cross")),
                }
            })
            .collect();
        positions.sort_by(|a, b| b.position_value.total_cmp(&a.position_value));
        Ok(DeskSnapshot {
            target: AccountSnapshot {
                account_value,
                positions: self
                    .positions
                    .iter()
                    .map(|(c, p)| (c.to_string(), p.size))
                    .collect(),
            },
            positions,
            orders: self.orders.clone(),
            mids: self.mids(),
            trades,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_wallet_trades_and_stays_consistent() {
        let mut demo = DemoSource::new(42);
        let mut trades = 0;
        for _ in 0..400 {
            let snap = demo.poll("x").unwrap();
            trades += snap.trades.len();
            for t in &snap.trades {
                let now = snap.target.positions.get(&t.coin).copied().unwrap_or(0.0);
                // Later trades in the same tick may move the position further.
                if snap.trades.iter().filter(|x| x.coin == t.coin).count() == 1 {
                    assert!(
                        (now - t.new_position()).abs() < 1e-9,
                        "{} {now} vs {}",
                        t.coin,
                        t.new_position()
                    );
                }
            }
            // Every position has exactly one TP and one SL covering it.
            for (coin, size) in &snap.target.positions {
                let tpsl: Vec<_> = snap
                    .orders
                    .iter()
                    .filter(|o| &o.coin == coin && o.is_position_tpsl)
                    .collect();
                assert_eq!(tpsl.len(), 2, "{coin}");
                assert!(
                    tpsl.iter()
                        .all(|o| (o.sz.parse::<f64>().unwrap() - size.abs()).abs() < 1e-9)
                );
            }
            assert!(snap.target.account_value > 0.0);
        }
        assert!(trades > 40, "only {trades} trades");
    }
}
