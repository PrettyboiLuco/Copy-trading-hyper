use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Local, TimeZone};
use clap::{Args, Parser, Subcommand, ValueEnum};

use copy_trading_hyper::DEFAULT_TARGET_WALLET;
use copy_trading_hyper::engine::{CopyConfig, CopyEngine, Sizing, TickReport};
use copy_trading_hyper::hyperliquid::{
    HyperliquidClient, MAINNET_URL, TESTNET_URL, position_details,
};
use copy_trading_hyper::journal::Journal;
use copy_trading_hyper::mock::{ScriptedSource, demo_scenario};
use copy_trading_hyper::paper::PaperAccount;
use copy_trading_hyper::types::Side;
use copy_trading_hyper::ws::{self, TradeEvent, TradeKind, WalletEvent};

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
        /// Only poll; do not react instantly to trades from the WebSocket feed.
        #[arg(long)]
        no_ws: bool,
    },
    /// Show the open positions and orders of the wallet once.
    Status {
        #[arg(long, env = "TARGET_WALLET", default_value = DEFAULT_TARGET_WALLET)]
        wallet: String,
        #[arg(long)]
        testnet: bool,
    },
    /// Show open positions/orders, then report every new trade and order live.
    Watch {
        #[arg(long, env = "TARGET_WALLET", default_value = DEFAULT_TARGET_WALLET)]
        wallet: String,
        #[arg(long)]
        testnet: bool,
        /// Also append every trade of the wallet to this CSV file.
        #[arg(long)]
        log: Option<PathBuf>,
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
            no_ws,
        } => {
            let account = if state.exists() {
                let acc = PaperAccount::load(&state)?;
                println!("Resuming paper account from {}", state.display());
                acc
            } else {
                copy.account()
            };
            let mut source = HyperliquidClient::new(api_url(testnet))?;
            let feed =
                (!no_ws).then(|| ws::spawn(ws::ws_url(api_url(testnet)), copy.wallet.clone()));
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
                    Err(err) => {
                        eprintln!("[{}] tick failed: {err:#}", Local::now().format("%H:%M:%S"))
                    }
                }
                n += 1;
                if ticks.is_some_and(|t| n >= t) {
                    break;
                }
                let wait = Duration::from_secs(interval);
                match &feed {
                    // Wake up early as soon as the target trades.
                    Some(rx) => wait_for_trade(rx, wait),
                    None => std::thread::sleep(wait),
                }
            }
        }
        Command::Status { wallet, testnet } => {
            let client = HyperliquidClient::new(api_url(testnet))?;
            print_wallet(&client, &wallet)?;
        }
        Command::Watch {
            wallet,
            testnet,
            log,
        } => {
            let client = HyperliquidClient::new(api_url(testnet))?;
            print_wallet(&client, &wallet)?;
            println!("\nWatching live (Ctrl+C to stop)...");
            let mut log = log.map(open_trade_log).transpose()?;
            for event in ws::spawn(ws::ws_url(api_url(testnet)), wallet) {
                print_event(&event);
                if let (WalletEvent::Trade(t), Some(file)) = (&event, log.as_mut()) {
                    writeln!(
                        file,
                        "{},{},{},{},{},{},{},{},{}",
                        Local
                            .timestamp_millis_opt(t.time_ms as i64)
                            .single()
                            .map_or_else(String::new, |d| d.to_rfc3339()),
                        t.coin,
                        trade_label(t),
                        t.side,
                        t.size,
                        t.avg_px,
                        t.new_position(),
                        t.closed_pnl,
                        t.oid
                    )?;
                }
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

/// Block until a trade of the target arrives or `max` has passed, printing
/// every feed event on the way.
fn wait_for_trade(rx: &Receiver<WalletEvent>, max: Duration) {
    let deadline = Instant::now() + max;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(event) => {
                print_event(&event);
                if matches!(event, WalletEvent::Trade(_)) {
                    // Pick up the rest of a burst before re-syncing.
                    while let Ok(more) = rx.recv_timeout(Duration::from_millis(300)) {
                        print_event(&more);
                    }
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => return,
            Err(RecvTimeoutError::Disconnected) => return std::thread::sleep(left),
        }
    }
}

fn open_trade_log(path: PathBuf) -> Result<std::fs::File> {
    let new = !path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    if new {
        writeln!(
            file,
            "time,coin,action,side,size,avg_px,position_after,closed_pnl,oid"
        )?;
    }
    Ok(file)
}

fn print_wallet(client: &HyperliquidClient, wallet: &str) -> Result<()> {
    let state = client.clearinghouse_state(wallet)?;
    let positions = position_details(&state);
    println!("Wallet {wallet}");
    println!("Account value: {} USDC", state.margin_summary.account_value);

    println!("\nOpen positions ({}):", positions.len());
    for p in &positions {
        let dir = if p.size > 0.0 { "LONG " } else { "SHORT" };
        let liq = p.liquidation_px.map_or("-".to_string(), |x| format!("{x}"));
        println!(
            "  {dir} {:<8} size {:>14.4}  entry {:>12}  value {:>12.2}  uPnL {:>+11.2}  {:<12} liq {}",
            p.coin,
            p.size,
            p.entry_px,
            p.position_value,
            p.unrealized_pnl,
            p.leverage.as_deref().unwrap_or("-"),
            liq
        );
    }
    if positions.is_empty() {
        println!("  (none)");
    }

    let orders = client.open_orders(wallet)?;
    println!("\nOpen orders ({}):", orders.len());
    for o in &orders {
        let side = if o.side == "B" { "BUY " } else { "SELL" };
        let trigger = if o.is_trigger {
            format!(" trigger {}", o.trigger_px)
        } else {
            String::new()
        };
        let tags = [
            (o.reduce_only, " reduce-only"),
            (o.is_position_tpsl, " TP/SL"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, t)| *t)
        .collect::<String>();
        println!(
            "  {side} {:<8} {:>12} @ {:<12} {:<20}{trigger}{tags}",
            o.coin, o.sz, o.limit_px, o.order_type
        );
    }
    if orders.is_empty() {
        println!("  (none)");
    }
    Ok(())
}

fn trade_label(t: &TradeEvent) -> String {
    let dir = |size: f64| if size > 0.0 { "LONG" } else { "SHORT" };
    match t.kind() {
        TradeKind::Open => format!("OPEN {}", dir(t.new_position())),
        TradeKind::Increase => format!("ADD {}", dir(t.new_position())),
        TradeKind::Reduce => format!("REDUCE {}", dir(t.start_position)),
        TradeKind::Close => format!("CLOSE {}", dir(t.start_position)),
        TradeKind::Flip => format!("FLIP TO {}", dir(t.new_position())),
    }
}

fn print_event(event: &WalletEvent) {
    let now = Local::now().format("%H:%M:%S");
    match event {
        WalletEvent::Connected => println!("[{now}] live feed connected"),
        WalletEvent::Disconnected(err) => {
            eprintln!("[{now}] live feed disconnected: {err} (reconnecting)")
        }
        WalletEvent::Trade(t) => {
            let time = Local
                .timestamp_millis_opt(t.time_ms as i64)
                .single()
                .map_or(now.to_string(), |d| d.format("%H:%M:%S").to_string());
            let pnl = if t.closed_pnl != 0.0 {
                format!("  closed PnL {:+.2}", t.closed_pnl)
            } else {
                String::new()
            };
            println!(
                "[{time}] TARGET {:<13} {:<8} {:>+12.4} @ {:<12} ({:.2} USDC) -> position {:+.4}{pnl}",
                trade_label(t),
                t.coin,
                t.signed_size(),
                t.avg_px,
                t.size * t.avg_px,
                t.new_position()
            );
        }
        WalletEvent::Order(o) => {
            let side = if o.side == Side::Buy { "BUY" } else { "SELL" };
            println!(
                "[{now}] TARGET order {:<10} {side} {} {} @ {} (oid {})",
                o.status, o.coin, o.size, o.limit_px, o.oid
            );
        }
    }
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
        if ignored.is_empty() {
            String::new()
        } else {
            format!(" | ignored: {}", ignored.join(","))
        }
    );
}
