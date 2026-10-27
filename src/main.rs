use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use clap::{Args, Parser, Subcommand, ValueEnum};

use copy_trading_hyper::DEFAULT_TARGET_WALLET;
use copy_trading_hyper::engine::{CopyConfig, CopyEngine, Sizing, TickReport};
use copy_trading_hyper::hyperliquid::{HyperliquidClient, MAINNET_URL, TESTNET_URL};
use copy_trading_hyper::journal::Journal;
use copy_trading_hyper::mock::{ScriptedSource, demo_scenario};
use copy_trading_hyper::paper::PaperAccount;
use copy_trading_hyper::source::MarketSource;

/// Hyperliquid copy-trading bot (paper trading only, no real orders).
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Follow the wallet live via the Hyperliquid API and paper-trade its positions.
    Run {
        #[command(flatten)]
        copy: CopyArgs,
        /// Polling interval in seconds.
        #[arg(long, default_value_t = 5)]
        interval: u64,
        /// Use the Hyperliquid testnet API.
        #[arg(long)]
        testnet: bool,
        /// Paper account state (resumed if the file exists).
        #[arg(long, default_value = "paper_state.json")]
        state: PathBuf,
        /// CSV log of simulated fills.
        #[arg(long, default_value = "trades.csv")]
        journal: PathBuf,
        /// Stop after this many ticks (default: run forever).
        #[arg(long)]
        ticks: Option<u64>,
    },
    /// Show the current positions of the wallet once.
    Status {
        #[arg(long, env = "TARGET_WALLET", default_value = DEFAULT_TARGET_WALLET)]
        wallet: String,
        #[arg(long)]
        testnet: bool,
    },
    /// Run an offline demo scenario (no network needed).
    Simulate {
        #[command(flatten)]
        copy: CopyArgs,
    },
}

#[derive(Args)]
struct CopyArgs {
    /// Wallet to follow.
    #[arg(long, env = "TARGET_WALLET", default_value = DEFAULT_TARGET_WALLET)]
    wallet: String,
    /// Starting balance of the paper account in USDC.
    #[arg(long, default_value_t = 1_000.0)]
    balance: f64,
    #[arg(long, value_enum, default_value_t = SizingMode::Proportional)]
    sizing: SizingMode,
    /// Multiplier for proportional sizing.
    #[arg(long, default_value_t = 1.0)]
    multiplier: f64,
    /// Ratio for fixed sizing (0.01 = copy 1% of the target size).
    #[arg(long, default_value_t = 0.01)]
    ratio: f64,
    #[arg(long, default_value_t = 5.0)]
    max_leverage: f64,
    #[arg(long, default_value_t = 10.0)]
    min_order_usd: f64,
    /// Maximum notional per coin in USDC.
    #[arg(long)]
    max_position_usd: Option<f64>,
    #[arg(long, default_value_t = 0.05)]
    rebalance_threshold: f64,
    /// Taker fee (fraction of notional).
    #[arg(long, default_value_t = 0.00045)]
    fee_rate: f64,
    #[arg(long, default_value_t = 5.0)]
    slippage_bps: f64,
    /// Do not copy positions the wallet already holds at startup.
    #[arg(long)]
    skip_existing: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum SizingMode {
    Proportional,
    Fixed,
}

impl CopyArgs {
    fn config(&self) -> CopyConfig {
        CopyConfig {
            wallet: self.wallet.clone(),
            sizing: match self.sizing {
                SizingMode::Proportional => Sizing::Proportional {
                    multiplier: self.multiplier,
                },
                SizingMode::Fixed => Sizing::Fixed { ratio: self.ratio },
            },
            max_leverage: self.max_leverage,
            min_order_usd: self.min_order_usd,
            max_position_usd: self.max_position_usd,
            rebalance_threshold: self.rebalance_threshold,
            skip_existing: self.skip_existing,
        }
    }

    fn account(&self) -> PaperAccount {
        PaperAccount::new(self.balance, self.fee_rate, self.slippage_bps)
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Run {
            copy,
            interval,
            testnet,
            state,
            journal,
            ticks,
        } => {
            let account = if state.exists() {
                let acc = PaperAccount::load(&state)?;
                println!("Resuming paper account from {}", state.display());
                acc
            } else {
                copy.account()
            };
            let mut source = HyperliquidClient::new(api_url(testnet))?;
            let mut engine = CopyEngine::new(copy.config(), account);
            let journal = Journal::open(&journal)?;
            println!(
                "Following {} (paper trading, start balance {:.2} USDC)",
                engine.config.wallet, engine.account.starting_balance
            );

            let mut n = 0;
            loop {
                match engine.tick(&mut source) {
                    Ok(report) => {
                        for fill in &report.fills {
                            journal.record(fill, report.equity)?;
                        }
                        print_report(&engine, &report);
                        engine.account.save(&state)?;
                    }
                    Err(err) => eprintln!("[{}] tick failed: {err:#}", Local::now().format("%H:%M:%S")),
                }
                n += 1;
                if ticks.is_some_and(|t| n >= t) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(interval));
            }
        }
        Command::Status { wallet, testnet } => {
            let mut source = HyperliquidClient::new(api_url(testnet))?;
            let snap = source.target_snapshot(&wallet)?;
            let mids = source.mids()?;
            println!("Wallet {wallet}");
            println!("Account value: {:.2} USDC", snap.account_value);
            let mut coins: Vec<_> = snap.positions.iter().collect();
            coins.sort_by(|a, b| a.0.cmp(b.0));
            for (coin, size) in coins {
                let px = mids.get(coin).copied().unwrap_or(f64::NAN);
                let dir = if *size > 0.0 { "LONG " } else { "SHORT" };
                println!("  {dir} {coin:<8} {size:>14.4}  @ {px:<12}  notional {:>12.2}", (size * px).abs());
            }
            if snap.positions.is_empty() {
                println!("  (no open perp positions)");
            }
        }
        Command::Simulate { copy } => {
            let mut source = ScriptedSource::new(demo_scenario());
            let mut engine = CopyEngine::new(copy.config(), copy.account());
            for _ in 0..source.len() {
                let report = engine.tick(&mut source)?;
                print_report(&engine, &report);
            }
            let acc = &engine.account;
            println!("\n=== Result ===");
            println!("Trades:        {}", acc.trade_count);
            println!("Realized PnL:  {:+.2} USDC", acc.realized_pnl);
            println!("Fees:          {:.2} USDC", acc.fees_paid);
            println!(
                "Balance:       {:.2} -> {:.2} USDC ({:+.2}%)",
                acc.starting_balance,
                acc.cash,
                (acc.cash / acc.starting_balance - 1.0) * 100.0
            );
        }
    }
    Ok(())
}

fn api_url(testnet: bool) -> &'static str {
    if testnet { TESTNET_URL } else { MAINNET_URL }
}

fn print_report(engine: &CopyEngine, report: &TickReport) {
    let ts = Local::now().format("%H:%M:%S");
    for f in &report.fills {
        println!(
            "[{ts}] {} {:<6} {:.6} @ {:.4}  fee {:.4}  pnl {:+.4}",
            f.side, f.coin, f.size, f.price, f.fee, f.realized_pnl
        );
    }
    let acc = &engine.account;
    let ignored = engine.ignored_coins();
    println!(
        "[{ts}] target {} pos / {:.0} USDC | paper equity {:.2} (uPnL {:+.2}, rPnL {:+.2}) notional {:.2} | {} pos{}",
        report.target.positions.len(),
        report.target.account_value,
        report.equity,
        acc.unrealized_pnl(&report.mids),
        acc.realized_pnl,
        report.notional,
        acc.positions.len(),
        if ignored.is_empty() { String::new() } else { format!(" | ignored: {}", ignored.join(",")) }
    );
}
