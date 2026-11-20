//! Synchronous fixture driver: feeds a fixture through the collectors into an `App` without threads, so tests,
//! `examples/peek.rs` and screenshots are deterministic.

use crate::app::App;
use crate::collect;
use crate::parse::iostat::split_stream;
use crate::source::{FixtureSource, Source};
use crate::workers::Msg;

pub struct Replay {
    pub src: FixtureSource,
    blocks: Vec<String>,
    tick: usize,
}

impl Replay {
    pub fn new(dir: &std::path::Path) -> Result<Self, String> {
        let src = FixtureSource::new(dir)?;
        let blocks = std::fs::read_to_string(dir.join("iostat.txt")).map(|t| split_stream(&t)).unwrap_or_default();
        Ok(Self { src, blocks, tick: 0 })
    }

    /// Config, disk inventory and SMART for every disk (what the slow workers would deliver).
    pub fn init(&mut self, app: &mut App) {
        let cfg = collect::config(&self.src);
        let pools = cfg.status.clone().unwrap_or_default();
        app.on_msg(Msg::Config(Box::new(cfg)));
        let inv = collect::inventory(&self.src, &pools);
        let devs: Vec<String> = inv.iter().map(|d| d.smart_dev.clone()).collect();
        app.on_msg(Msg::Disks(Ok(inv)));
        app.actions.clear();
        for d in devs {
            let r = collect::smart(&self.src, &d);
            app.on_msg(Msg::Smart(d, r));
        }
    }

    /// One sampling interval: kstats, the iostat block of the same tick, histograms every 5 ticks.
    pub fn step(&mut self, app: &mut App) {
        self.src.set_tick(self.tick);
        app.on_msg(Msg::Kstats(collect::kstats(&self.src)));
        if !self.blocks.is_empty() && self.tick > 0 {
            let b = self.blocks[(self.tick - 1) % self.blocks.len()].clone();
            app.on_msg(Msg::Iostat(Ok(b), self.src.now()));
        }
        if self.tick.is_multiple_of(5) {
            app.on_msg(Msg::Hists(collect::hists(&self.src), self.src.now()));
        }
        self.tick += 1;
    }

    /// Run the app's queued requests synchronously (what the job worker would do).
    pub fn serve(&self, app: &mut App) {
        use crate::app::Action;
        for a in std::mem::take(&mut app.actions) {
            match a {
                Action::Snapshots(ds) => {
                    let r = collect::snapshots(&self.src, &ds);
                    app.on_msg(Msg::Snapshots(ds, r));
                }
                Action::History(p) => {
                    let r = collect::history(&self.src, &p);
                    app.on_msg(Msg::History(p, r));
                }
                Action::Smart(d) => {
                    let r = collect::smart(&self.src, &d);
                    app.on_msg(Msg::Smart(d, r));
                }
                _ => {}
            }
        }
    }

    pub fn run(&mut self, app: &mut App, ticks: usize) {
        for _ in 0..ticks {
            self.step(app);
        }
    }
}
