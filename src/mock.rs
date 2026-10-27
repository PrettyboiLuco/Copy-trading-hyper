use anyhow::{Result, bail};

use crate::source::MarketSource;
use crate::types::{AccountSnapshot, Mids};

/// One tick of a scripted scenario.
#[derive(Debug, Clone)]
pub struct ScriptStep {
    pub target: AccountSnapshot,
    pub mids: Mids,
}

/// Offline source that replays a fixed list of steps. Each call to
/// `target_snapshot` advances to the next step; `mids` returns the prices of
/// the current step.
pub struct ScriptedSource {
    steps: Vec<ScriptStep>,
    current: Option<usize>,
}

impl ScriptedSource {
    pub fn new(steps: Vec<ScriptStep>) -> Self {
        Self {
            steps,
            current: None,
        }
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

impl MarketSource for ScriptedSource {
    fn target_snapshot(&mut self, _wallet: &str) -> Result<AccountSnapshot> {
        let next = self.current.map_or(0, |i| i + 1);
        if next >= self.steps.len() {
            bail!("scenario exhausted");
        }
        self.current = Some(next);
        Ok(self.steps[next].target.clone())
    }

    fn mids(&mut self) -> Result<Mids> {
        match self.current {
            Some(i) => Ok(self.steps[i].mids.clone()),
            None => bail!("target_snapshot must be called first"),
        }
    }
}

pub fn mids(prices: &[(&str, f64)]) -> Mids {
    prices.iter().map(|(c, p)| (c.to_string(), *p)).collect()
}

/// Demo scenario used by `simulate`: the target (100k account) opens BTC long,
/// adds ETH short, scales BTC, flips ETH and finally closes everything.
pub fn demo_scenario() -> Vec<ScriptStep> {
    let step = |value, pos: &[(&str, f64)], px: &[(&str, f64)]| ScriptStep {
        target: AccountSnapshot::new(value, pos),
        mids: mids(px),
    };
    vec![
        step(100_000.0, &[], &[("BTC", 60_000.0), ("ETH", 3_000.0)]),
        step(100_000.0, &[("BTC", 1.0)], &[("BTC", 60_000.0), ("ETH", 3_000.0)]),
        step(101_000.0, &[("BTC", 1.0), ("ETH", -10.0)], &[("BTC", 61_000.0), ("ETH", 3_000.0)]),
        step(103_000.0, &[("BTC", 1.5), ("ETH", -10.0)], &[("BTC", 62_000.0), ("ETH", 2_900.0)]),
        step(104_500.0, &[("BTC", 1.5), ("ETH", 5.0)], &[("BTC", 62_500.0), ("ETH", 2_850.0)]),
        step(105_000.0, &[("BTC", 0.5), ("ETH", 5.0)], &[("BTC", 63_000.0), ("ETH", 2_950.0)]),
        step(106_000.0, &[], &[("BTC", 63_500.0), ("ETH", 3_050.0)]),
    ]
}
