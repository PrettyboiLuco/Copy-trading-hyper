use std::collections::HashMap;

/// Mid price per coin.
pub type Mids = HashMap<String, f64>;

/// Snapshot of the followed wallet: account value and signed position size per coin
/// (positive = long, negative = short).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountSnapshot {
    pub account_value: f64,
    pub positions: HashMap<String, f64>,
}

impl AccountSnapshot {
    pub fn new(account_value: f64, positions: &[(&str, f64)]) -> Self {
        Self {
            account_value,
            positions: positions
                .iter()
                .map(|(coin, size)| (coin.to_string(), *size))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn of(delta: f64) -> Self {
        if delta >= 0.0 { Side::Buy } else { Side::Sell }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        })
    }
}

/// A simulated execution on the paper account.
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub coin: String,
    pub side: Side,
    pub size: f64,
    pub price: f64,
    pub fee: f64,
    pub realized_pnl: f64,
}
