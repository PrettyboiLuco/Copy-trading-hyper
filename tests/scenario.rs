use copy_trading_hyper::DEFAULT_TARGET_WALLET;
use copy_trading_hyper::engine::{CopyConfig, CopyEngine};
use copy_trading_hyper::mock::{ScriptedSource, demo_scenario};
use copy_trading_hyper::paper::PaperAccount;
use copy_trading_hyper::source::MarketSource;

#[test]
fn demo_scenario_mirrors_target() {
    let steps = demo_scenario();
    let mut source = ScriptedSource::new(steps.clone());
    let mut engine = CopyEngine::new(
        CopyConfig::new(DEFAULT_TARGET_WALLET),
        PaperAccount::new(1_000.0, 0.00045, 5.0),
    );

    for step in &steps {
        let report = engine.tick(&mut source).unwrap();
        // Every coin of the target is held in the same direction, and nothing else.
        for (coin, size) in &step.target.positions {
            let mine = engine.account.size(coin);
            assert_eq!(mine.signum(), size.signum(), "{coin} direction");
            let ratio = mine / size;
            let expected = report.equity / step.target.account_value;
            assert!((ratio / expected - 1.0).abs() < 0.1, "{coin} size off: {ratio} vs {expected}");
        }
        for coin in engine.account.positions.keys() {
            assert!(step.target.positions.contains_key(coin), "stale position {coin}");
        }
    }

    // Target ends flat and was profitable -> so are we, after fees.
    assert!(engine.account.positions.is_empty());
    assert!(engine.account.cash > 1_000.0);
    assert!(engine.account.fees_paid > 0.0);
    assert!(source.target_snapshot(DEFAULT_TARGET_WALLET).is_err());
}

