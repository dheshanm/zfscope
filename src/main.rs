use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use color_eyre::eyre::{Result, eyre};
use crossbeam_channel::{select, tick, unbounded};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use zfscope::app::{Action, App};
use zfscope::source::SourceSpec;

/// Live, read-only view of an OpenZFS host: pool/vdev/disk IO and latency, datasets and space, ARC, ZIL, TXGs,
/// properties and disks. Run it on the ZFS host itself (as root for zpool/smartctl; kstats work as any user).
#[derive(Parser, Debug)]
#[command(version = zfscope::VERSION, about)]
struct Args {
    /// Sampling interval in seconds (1-60; `+`/`-` change it while running).
    #[arg(short, long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=60))]
    interval: u64,

    /// Replay a captured fixture directory instead of the live system (see scripts/capture-fixture.sh).
    #[arg(long, value_name = "DIR")]
    fixture: Option<PathBuf>,

    /// Log file (stdout belongs to the TUI), created readable by its owner only. Filter with RUST_LOG.
    #[arg(long, default_value = "/var/log/zfscope.log")]
    log: PathBuf,

    /// Don't run smartctl (the Disks view then shows sysfs data only).
    #[arg(long)]
    no_smart: bool,

    /// Don't capture the mouse (keeps the terminal's native text selection).
    #[arg(long)]
    no_mouse: bool,

    /// Print what zfscope collects (one kstat sample, config, inventory) as text and exit — no TUI.
    #[arg(long, hide = true)]
    dump: bool,
}

fn init_logging(path: &std::path::Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
        #[cfg(target_os = "linux")]
        opts.custom_flags(0o400000); // O_NOFOLLOW
    }
    let opened = opts.open(path);
    let open_error = opened.as_ref().err().map(|e| e.to_string());
    let file_layer = opened.ok().map(|f| {
        let (w, guard) = tracing_appender::non_blocking(f);
        (tracing_subscriber::fmt::layer().with_writer(w).with_ansi(false).with_filter(filter()), guard)
    });
    let (file_layer, guard) = match file_layer {
        Some((l, g)) => (Some(l), Some(g)),
        None => (None, None),
    };
    tracing_subscriber::registry()
        .with(file_layer)
        .with(tui_logger::TuiTracingSubscriberLayer.with_filter(filter()))
        .init();
    let _ = tui_logger::init_logger(tui_logger::LevelFilter::Trace);
    if let Some(e) = open_error {
        tracing::warn!("not logging to {}: {e} (use --log PATH)", path.display());
    }
    guard
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "localhost".into())
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();
    let spec = match &args.fixture {
        Some(d) => SourceSpec::Fixture(d.clone()),
        None => SourceSpec::Live,
    };
    let src = spec.build().map_err(|e| eyre!(e))?;
    if args.dump {
        return zfscope::dump::dump(&*src, Duration::from_secs(args.interval));
    }
    let _guard = init_logging(&args.log);
    let host = match &args.fixture {
        Some(d) => {
            let meta: serde_json::Value = std::fs::read_to_string(d.join("meta.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            format!("{} (replay)", meta["host"].as_str().unwrap_or("fixture"))
        }
        None => hostname(),
    };
    tracing::info!("starting zfscope {}: {}, interval {}s", zfscope::VERSION, src.describe(), args.interval);

    let interval = Duration::from_secs(args.interval);
    let (tx, rx) = unbounded();
    let workers = zfscope::workers::start(src.clone(), interval, tx, !args.no_smart);
    let mut app = App::new(host, src.describe(), interval);

    let (key_tx, key_rx) = unbounded();
    std::thread::Builder::new().name("input".into()).spawn(move || {
        while let Ok(ev) = event::read() {
            if key_tx.send(ev).is_err() {
                break;
            }
        }
    })?;
    // redraw at least once a second (ages, stale banners) even when nothing arrives
    let redraw = tick(Duration::from_millis(1000));

    let mut terminal = ratatui::init();
    if !args.no_mouse {
        execute!(std::io::stdout(), EnableMouseCapture)?;
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
            prev(info);
        }));
    }
    let res: Result<()> = (|| {
        loop {
            terminal.draw(|f| zfscope::ui::draw(f, &mut app))?;
            select! {
                recv(key_rx) -> ev => match ev? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => app.handle_key(k),
                    Event::Mouse(m) => app.handle_mouse(m),
                    _ => {}
                },
                recv(rx) -> m => app.on_msg(m?),
                recv(redraw) -> _ => {}
            }
            // coalesce bursts (iostat block + kstat sample arrive together) into one redraw
            while let Ok(m) = rx.try_recv() {
                app.on_msg(m);
            }
            for a in app.actions.drain(..) {
                match a {
                    Action::SetInterval(d) => workers.set_interval(d),
                    Action::Refresh => workers.refresh(),
                    Action::Snapshots(ds) => workers.job(zfscope::workers::Job::Snapshots(ds)),
                    Action::History(pool) => workers.job(zfscope::workers::Job::History(pool)),
                    Action::Smart(dev) => workers.job(zfscope::workers::Job::Smart(dev)),
                    Action::SmartDevices(devs) => workers.smart_devices(devs),
                    Action::MouseCapture(on) if !args.no_mouse => {
                        let _ = if on {
                            execute!(std::io::stdout(), EnableMouseCapture)
                        } else {
                            execute!(std::io::stdout(), DisableMouseCapture)
                        };
                    }
                    Action::MouseCapture(_) => {}
                }
            }
            if app.quit {
                return Ok(());
            }
        }
    })();
    if !args.no_mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    res
}
