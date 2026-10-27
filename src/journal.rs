use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;

use crate::types::Fill;

/// Append-only CSV log of simulated fills.
pub struct Journal {
    path: PathBuf,
}

impl Journal {
    const HEADER: &'static str =
        "timestamp,coin,side,size,price,notional,fee,realized_pnl,equity_after";

    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            std::fs::write(path, format!("{}\n", Self::HEADER))?;
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    pub fn record(&self, fill: &Fill, equity_after: f64) -> Result<()> {
        let mut file = OpenOptions::new().append(true).open(&self.path)?;
        writeln!(
            file,
            "{},{},{},{},{},{:.2},{:.4},{:.4},{:.2}",
            Utc::now().to_rfc3339(),
            fill.coin,
            fill.side,
            fill.size,
            fill.price,
            fill.size * fill.price,
            fill.fee,
            fill.realized_pnl,
            equity_after
        )?;
        Ok(())
    }
}
