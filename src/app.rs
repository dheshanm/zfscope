//! App state and input handling. No I/O: requests for the workers go out through `actions`.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::model::{Model, vkey};
use crate::parse::zjson::{DsKind, Prop, Props, PropsExt, Vdev};
use crate::workers::Msg;
use crate::write::{Change, Target};

/// How long a write-mode notice stays on screen.
pub const NOTICE_FOR: Duration = Duration::from_secs(10);

pub const INTERVALS: [u64; 6] = [1, 2, 5, 10, 30, 60];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Pool,
    Datasets,
    Arc,
    Zil,
    Disks,
    Props,
}

impl Tab {
    pub const ALL: [Tab; 7] = [Tab::Overview, Tab::Pool, Tab::Datasets, Tab::Arc, Tab::Zil, Tab::Disks, Tab::Props];
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Pool => "Pool",
            Tab::Datasets => "Datasets",
            Tab::Arc => "ARC",
            Tab::Zil => "ZIL·TXG",
            Tab::Disks => "Disks",
            Tab::Props => "Props",
        }
    }
    fn idx(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// Topology table column sets (`m`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoMode {
    Io,
    Latency,
    Queue,
    Space,
}

impl IoMode {
    fn next(self) -> Self {
        match self {
            IoMode::Io => IoMode::Latency,
            IoMode::Latency => IoMode::Queue,
            IoMode::Queue => IoMode::Space,
            IoMode::Space => IoMode::Io,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            IoMode::Io => "IO",
            IoMode::Latency => "latency",
            IoMode::Queue => "queues",
            IoMode::Space => "space",
        }
    }
}

/// Topology sort (click a header): siblings are reordered within their parent; class sections stay last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopoSort {
    Order,
    Name,
    State,
    Errors,
    Rops,
    Wops,
    Rbw,
    Wbw,
    Wait(usize),
    Active,
    Pending,
    Queue(usize),
    Size,
    Alloc,
    Free,
    Cap,
    Frag,
}

impl TopoSort {
    pub fn default_desc(self) -> bool {
        !matches!(self, TopoSort::Order | TopoSort::Name | TopoSort::State)
    }
    pub fn title(self) -> String {
        match self {
            TopoSort::Order => "pool order".into(),
            TopoSort::Wait(i) => format!("{} wait", crate::parse::iostat::w::NAMES[i]),
            TopoSort::Queue(i) => format!("{} queue", crate::parse::iostat::QUEUE_NAMES[i]),
            other => format!("{other:?}").to_lowercase(),
        }
    }
}

/// Which column of the Props tab has keyboard focus. Enter moves right, Esc / ← moves left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropsFocus {
    Targets,
    Props,
    About,
}

/// Property table sort (click a header; third click returns to ZFS's own order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropSort {
    Order,
    Name,
    Value,
    Source,
}

/// Disks table sort (click a header; third click returns to pool order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskSort {
    Order,
    Slot,
    Vdev,
    Dev,
    Model,
    Size,
    Temp,
    Hours,
    Defects,
    Errors,
    Util,
    Await,
    Rops,
    Wops,
    Wait,
}

impl DiskSort {
    pub fn default_desc(self) -> bool {
        !matches!(self, DiskSort::Order | DiskSort::Slot | DiskSort::Vdev | DiskSort::Dev | DiskSort::Model)
    }
}

/// Compare strings with embedded numbers naturally (e0s2 < e0s10, sdb < sdaa).
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut txt = String::new();
        let mut num = String::new();
        for c in s.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                if !num.is_empty() {
                    out.push((std::mem::take(&mut txt), num.parse().unwrap_or(0)));
                    num.clear();
                }
                txt.push(c);
            }
        }
        out.push((txt, num.parse().unwrap_or(0)));
        out
    };
    key(a).cmp(&key(b)).then_with(|| a.len().cmp(&b.len()))
}

/// What the disk grid colours by (`c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridMetric {
    /// disk_wait, the worse of read and write
    Latency,
    DiskRead,
    DiskWrite,
    /// total_wait (queued in ZFS + on the disk), the worse of read and write
    TotalWait,
    Ops,
    ReadOps,
    WriteOps,
    ReadBw,
    WriteBw,
    Util,
    /// IOs issued to the disk and not yet completed, all queues
    Queue,
    Errors,
    Temp,
    Defects,
    Wear,
}

impl GridMetric {
    /// Every metric, in the order the grid's metric panel lists them (and `c` cycles), grouped by `section`.
    pub const ALL: [GridMetric; 15] = [
        GridMetric::Latency,
        GridMetric::DiskRead,
        GridMetric::DiskWrite,
        GridMetric::TotalWait,
        GridMetric::Ops,
        GridMetric::ReadOps,
        GridMetric::WriteOps,
        GridMetric::ReadBw,
        GridMetric::WriteBw,
        GridMetric::Util,
        GridMetric::Queue,
        GridMetric::Errors,
        GridMetric::Temp,
        GridMetric::Defects,
        GridMetric::Wear,
    ];

    pub fn index(self) -> usize {
        GridMetric::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
    fn next(self) -> Self {
        GridMetric::ALL[(self.index() + 1) % GridMetric::ALL.len()]
    }
    pub fn title(self) -> &'static str {
        match self {
            GridMetric::Latency => "disk wait",
            GridMetric::DiskRead => "disk wait read",
            GridMetric::DiskWrite => "disk wait write",
            GridMetric::TotalWait => "total wait",
            GridMetric::Ops => "IOPS",
            GridMetric::ReadOps => "read ops/s",
            GridMetric::WriteOps => "write ops/s",
            GridMetric::ReadBw => "read bandwidth",
            GridMetric::WriteBw => "write bandwidth",
            GridMetric::Util => "util %",
            GridMetric::Queue => "active queue",
            GridMetric::Errors => "errors",
            GridMetric::Temp => "temperature",
            GridMetric::Defects => "defects",
            GridMetric::Wear => "SSD wear",
        }
    }
    /// Heading the metric is listed under in the metric panel.
    pub fn section(self) -> &'static str {
        match self {
            GridMetric::Latency | GridMetric::DiskRead | GridMetric::DiskWrite | GridMetric::TotalWait => "latency",
            GridMetric::Ops | GridMetric::ReadOps | GridMetric::WriteOps | GridMetric::ReadBw | GridMetric::WriteBw => {
                "IO"
            }
            GridMetric::Util | GridMetric::Queue => "load",
            GridMetric::Errors | GridMetric::Temp | GridMetric::Defects | GridMetric::Wear => "health",
        }
    }
    /// What the metric measures (shown under the grid's metric panel list).
    pub fn about(self) -> &'static str {
        match self {
            GridMetric::Latency => "Time each IO spent on the disk itself (the worse of reads and writes).",
            GridMetric::DiskRead => "Time each read spent on the disk itself.",
            GridMetric::DiskWrite => "Time each write spent on the disk itself.",
            GridMetric::TotalWait => {
                "Time each IO took in all, queued in ZFS plus on the disk (the worse of reads and writes): \
                 what applications feel."
            }
            GridMetric::Ops => "Reads + writes per second sent to the disk.",
            GridMetric::ReadOps => "Reads per second sent to the disk.",
            GridMetric::WriteOps => "Writes per second sent to the disk.",
            GridMetric::ReadBw => "Bytes per second read from the disk.",
            GridMetric::WriteBw => "Bytes per second written to the disk.",
            GridMetric::Util => "Share of time the disk was busy with at least one IO.",
            GridMetric::Queue => "IOs ZFS has issued to the disk that haven't completed yet (all queues).",
            GridMetric::Errors => "Read / write / checksum errors plus slow IOs. Red also marks disks not ONLINE.",
            GridMetric::Temp => "Drive temperature from SMART, coloured on a fixed 25 → 60 °C scale.",
            GridMetric::Defects => "SMART grown defects (SAS) or reallocated sectors (SATA). Red = any.",
            GridMetric::Wear => "Share of rated endurance used, from SMART (SSD / NVMe; grey = not reported).",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsSort {
    Name,
    Used,
    Avail,
    Refer,
    Snap,
    Lused,
    Ratio,
    Quota,
    Read,
    Write,
    Rops,
    Wops,
    Unlinks,
    ZilCommits,
    ZilBytes,
}

impl DsSort {
    /// `s` cycles through these (tree / flat ztop mode).
    const TREE: [DsSort; 7] =
        [DsSort::Name, DsSort::Used, DsSort::Refer, DsSort::Snap, DsSort::Quota, DsSort::Read, DsSort::Write];
    const FLAT: [DsSort; 8] = [
        DsSort::Read,
        DsSort::Write,
        DsSort::Rops,
        DsSort::Wops,
        DsSort::Unlinks,
        DsSort::ZilCommits,
        DsSort::ZilBytes,
        DsSort::Name,
    ];
    fn next(self, flat: bool) -> Self {
        let order: &[DsSort] = if flat { &Self::FLAT } else { &Self::TREE };
        let i = order.iter().position(|k| *k == self).map(|i| i + 1).unwrap_or(0);
        order[i % order.len()]
    }
    /// Names sort ascending by default, numbers descending (biggest first).
    pub fn default_desc(self) -> bool {
        self != DsSort::Name
    }
    pub fn title(self) -> &'static str {
        match self {
            DsSort::Name => "name",
            DsSort::Used => "used",
            DsSort::Avail => "avail",
            DsSort::Refer => "refer",
            DsSort::Snap => "snapshots",
            DsSort::Lused => "logical used",
            DsSort::Ratio => "ratio",
            DsSort::Quota => "quota fill",
            DsSort::Read => "read B/s",
            DsSort::Write => "write B/s",
            DsSort::Rops => "read ops",
            DsSort::Wops => "write ops",
            DsSort::Unlinks => "unlinks",
            DsSort::ZilCommits => "ZIL commits",
            DsSort::ZilBytes => "ZIL bytes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsPane {
    Space,
    Io,
    Snapshots,
    Props,
}

impl DsPane {
    pub const ALL: [DsPane; 4] = [DsPane::Space, DsPane::Io, DsPane::Snapshots, DsPane::Props];
    pub fn title(self) -> &'static str {
        match self {
            DsPane::Space => "Space",
            DsPane::Io => "IO·ZIL",
            DsPane::Snapshots => "Snapshots",
            DsPane::Props => "Properties",
        }
    }
}

/// One visible row of the topology tree.
#[derive(Debug, Clone, PartialEq)]
pub struct TopoRow {
    pub key: String,
    pub pool: String,
    pub name: String,
    pub depth: usize,
    pub vtype: String,
    pub has_children: bool,
    pub collapsed: bool,
    /// tree guides: for each ancestor level, whether more siblings follow (draw │) — plus whether this is last
    pub guides: Vec<bool>,
    pub last: bool,
}

/// One visible row of the dataset tree / flat list.
#[derive(Debug, Clone, PartialEq)]
pub struct DsRow {
    pub name: String,
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    SetInterval(Duration),
    Refresh,
    Snapshots(String),
    History(String),
    Smart(String),
    SmartDevices(Vec<String>),
    /// capture (true) or release (false) the mouse
    MouseCapture(bool),
    /// run a confirmed write-mode change
    Write(Change),
}

/// Write mode: editing a property value (or the name of a new user property, then its value).
#[derive(Debug, Clone, PartialEq)]
pub struct EditState {
    pub target: Target,
    /// empty while naming a new user property
    pub key: String,
    pub input: String,
    /// the value the input started from (submitting it unchanged does nothing)
    pub orig: String,
    /// highlighted entry of the possible values (↑↓)
    pub opt_sel: Option<usize>,
    /// typing the name of a new user property
    pub naming: bool,
    pub new_prop: bool,
    pub error: Option<String>,
}

const FEATURE_OPTS: crate::help::Opts =
    &[("enabled", "make the feature available; it becomes active when first used (one-way)")];

impl EditState {
    /// Known values for this property, each with a description.
    pub fn options(&self) -> crate::help::Opts {
        if self.naming { &[] } else { prop_options(&self.target, &self.key) }
    }
}

/// Values that can be set for a property (feature flags: only `enabled`).
pub fn prop_options(target: &Target, key: &str) -> crate::help::Opts {
    if key.starts_with("feature@") {
        FEATURE_OPTS
    } else if target.is_pool() {
        crate::help::pool_options(key).unwrap_or(&[])
    } else {
        crate::help::dataset_options(key).unwrap_or(&[])
    }
}

/// What an option label puts in the input: patterns (`zstd-N`) leave the number to type, placeholders nothing.
pub fn option_fill(label: &str) -> String {
    if let Some(p) = label.strip_suffix('N').filter(|p| p.ends_with('-')) {
        p.to_string()
    } else if label.starts_with('<') {
        String::new()
    } else {
        label.to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmState {
    pub change: Change,
    /// the property as it is now (None for a new user property)
    pub old: Option<Prop>,
    /// one-way changes: the pool name typed so far
    pub typed: String,
    pub error: Option<String>,
}

/// A dialog that takes all keys until it closes.
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    /// "enable write mode?"
    Unlock,
    Edit(EditState),
    Confirm(ConfirmState),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Tab(Tab),
    /// a row of the Props tab's pools & datasets column
    TargetRow(usize),
    /// the "about this property" pane
    AboutArea,
    PropSort(PropSort),
    DiskSort(DiskSort),
    /// a row of the detail pane (snapshots / properties) under a list
    PaneRow(usize),
    /// anywhere over that detail pane (wheel scrolling goes to the pane)
    PaneArea,
    TopoSort(TopoSort),
    DsSort(DsSort),
    /// fold/unfold the tree row with this index
    Fold(usize),
    /// row index in the current tab's main list
    Row(usize),
    SubTab(usize),
    /// an entry of the disk grid's metric panel
    GridMetric(GridMetric),
}

#[derive(Default, Debug)]
pub struct HitMap(pub Vec<(Rect, Hit)>);

impl HitMap {
    pub fn add(&mut self, r: Rect, h: Hit) {
        self.0.push((r, h));
    }
    fn at(&self, x: u16, y: u16) -> Option<Hit> {
        self.0.iter().rev().find(|(r, _)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom()).map(|(_, h)| *h)
    }
}

pub struct App {
    pub model: Model,
    pub host: String,
    pub source: String,
    pub tab: Tab,
    pub interval: Duration,
    pub paused: bool,
    /// newest kstat sample received while paused (applied as the new baseline on resume)
    pub held_kstats: Option<crate::collect::KSample>,
    /// other worker results received while paused, applied on resume
    pub held: Vec<Msg>,
    pub quit: bool,
    pub show_help: bool,
    pub help_scroll: usize,
    pub show_log: bool,
    pub actions: Vec<Action>,
    pub started: Instant,
    pub last_kstat: Option<Instant>,
    pub last_config: Option<Instant>,
    /// `/` filter text (per tab, applies to the main list); `editing` while typing
    pub filter: String,
    pub editing: bool,

    // Pool tab
    pub topo_sel: Option<String>,
    pub topo_collapsed: HashSet<String>,
    pub io_mode: IoMode,
    pub grid: bool,
    pub grid_metric: GridMetric,
    pub heat_disk: bool,
    pub topo_sort: TopoSort,
    pub topo_desc: bool,
    pub pool_history: bool,
    pub hist_sel: usize,

    // Datasets tab
    pub ds_sel: Option<String>,
    pub ds_collapsed: HashSet<String>,
    pub ds_flat: bool,
    pub ds_sort: DsSort,
    pub ds_desc: bool,
    pub ds_detail: bool,
    pub ds_pane: DsPane,
    /// keyboard focus is on the detail pane (not the dataset list)
    pub ds_focus_pane: bool,
    pub snap_sel: usize,

    // Disks tab
    pub disk_sel: usize,
    pub disk_detail: bool,
    pub disk_sort: DiskSort,
    pub disk_desc: bool,
    pub disk_scroll: usize,

    // Props tab
    pub props_target: Option<String>,
    pub props_local: bool,
    /// keyboard focus is on the "about this property" pane (Props tab, or the Datasets Properties pane)
    pub about_focus: bool,
    pub about_scroll: usize,
    /// property the about pane last showed (scroll resets when it changes)
    pub about_key: String,
    pub props_sort: PropSort,
    pub props_desc: bool,
    pub props_sel: usize,
    /// selected row of the Props tab's pools & datasets column
    pub picker_sel: usize,
    pub props_focus: PropsFocus,
    /// after switching pool/dataset (or re-sorting), re-select the property that was selected (by name)
    pub props_relocate: bool,
    /// filter for the pools & datasets column (the property list uses `filter`)
    pub target_filter: String,
    /// `/` was pressed in the pools & datasets column: typing edits `target_filter`
    pub filter_targets: bool,

    pub scroll: usize,
    /// selected tunable in the ARC / ZIL·TXG parameter lists
    pub param_sel: usize,
    /// pool shown in the ZIL·TXG tab (index into pools; `n` cycles)
    pub txg_pool: usize,
    pub hits: HitMap,
    last_click: Option<(Instant, u16, u16)>,
    smart_sent: Vec<String>,

    // Write mode
    /// write mode can be unlocked (root and not --read-only; fixtures: dry run)
    pub can_write: bool,
    /// why it can't (shown when W is pressed)
    pub write_off_reason: &'static str,
    /// unlocked with W
    pub rw: bool,
    pub modal: Option<Modal>,
    /// a change is running
    pub write_busy: bool,
    /// (when, text, is_error): result of the last write-mode action
    pub notice: Option<(Instant, String, bool)>,
    /// property selected in the property table that was drawn last (None when it shows no rows)
    pub props_sel_key: Option<String>,
}

impl App {
    pub fn new(host: String, source: String, interval: Duration) -> Self {
        Self {
            model: Model::new(),
            host,
            source,
            tab: Tab::Overview,
            interval,
            paused: false,
            held_kstats: None,
            held: Vec::new(),
            quit: false,
            show_help: false,
            help_scroll: 0,
            show_log: false,
            actions: Vec::new(),
            started: Instant::now(),
            last_kstat: None,
            last_config: None,
            filter: String::new(),
            editing: false,
            topo_sel: None,
            topo_collapsed: HashSet::new(),
            io_mode: IoMode::Io,
            grid: false,
            grid_metric: GridMetric::Latency,
            heat_disk: false,
            topo_sort: TopoSort::Order,
            topo_desc: false,
            pool_history: false,
            hist_sel: 0,
            ds_sel: None,
            ds_collapsed: HashSet::new(),
            ds_flat: false,
            ds_sort: DsSort::Name,
            ds_desc: false,
            ds_detail: false,
            ds_pane: DsPane::Space,
            ds_focus_pane: false,
            snap_sel: 0,
            disk_sel: 0,
            disk_detail: false,
            disk_sort: DiskSort::Order,
            disk_desc: false,
            disk_scroll: 0,
            props_target: None,
            props_local: false,
            about_focus: false,
            about_scroll: 0,
            about_key: String::new(),
            props_sort: PropSort::Order,
            props_desc: false,
            props_sel: 0,
            picker_sel: 0,
            props_focus: PropsFocus::Targets,
            props_relocate: false,
            target_filter: String::new(),
            filter_targets: false,
            scroll: 0,
            param_sel: 0,
            txg_pool: 0,
            hits: HitMap::default(),
            last_click: None,
            smart_sent: Vec::new(),
            can_write: false,
            write_off_reason: "write mode needs root: run zfscope as root",
            rw: false,
            modal: None,
            write_busy: false,
            notice: None,
            props_sel_key: None,
        }
    }

    // ---------------------------------------------------------------- worker messages

    pub fn on_msg(&mut self, m: Msg) {
        // the answer to something the user just did: show it even while paused
        let m = match m {
            Msg::Wrote(c, r) => return self.on_wrote(c, r),
            m => m,
        };
        // Paused: the screen is frozen (so a text selection isn't wiped by redraws). Keep only the newest kstat
        // sample and config to re-baseline from on resume; interval data (iostat blocks, histograms) is dropped.
        if self.paused {
            match m {
                Msg::Kstats(s) => {
                    self.last_kstat = Some(Instant::now());
                    self.held_kstats = Some(s);
                }
                Msg::Iostat(..) | Msg::Hists(..) => {}
                Msg::Config(c) => {
                    self.held.retain(|h| !matches!(h, Msg::Config(_)));
                    self.held.push(Msg::Config(c));
                }
                other => self.held.push(other),
            }
            return;
        }
        match m {
            Msg::Kstats(s) => {
                self.model.on_kstats(s, true);
                self.last_kstat = Some(Instant::now());
            }
            Msg::Iostat(b, t) => self.model.on_iostat(b, t, true),
            Msg::Hists(h, t) => self.model.on_hists(h, t, true),
            Msg::Config(c) => {
                self.model.on_config(*c);
                self.last_config = Some(Instant::now());
                if self.topo_sel.is_none() {
                    self.topo_sel = self.model.pools().first().map(|p| vkey(&p.name, &p.name));
                }
                if self.ds_sel.is_none() {
                    self.ds_sel = self.model.datasets().first().map(|d| d.name.clone());
                }
            }
            Msg::Disks(d) => {
                self.model.on_disks(d);
                let devs: Vec<String> = self.model.disks.iter().map(|d| d.smart_dev.clone()).collect();
                if devs != self.smart_sent {
                    self.smart_sent = devs.clone();
                    self.actions.push(Action::SmartDevices(devs));
                }
            }
            Msg::Smart(dev, s) => self.model.on_smart(dev, s),
            Msg::Snapshots(ds, s) => self.model.on_snapshots(ds, s),
            Msg::History(pool, h) => self.model.on_history(pool, h),
            Msg::Wrote(..) => unreachable!("handled above"),
        }
    }

    fn on_wrote(&mut self, c: Change, r: crate::source::Res<String>) {
        self.write_busy = false;
        match r {
            Ok(out) => {
                let out = out.trim();
                let note = if out.is_empty() { String::new() } else { format!(" · {out}") };
                self.note(false, format!("✓ {}{note}", c.command_line()));
                self.actions.push(Action::Refresh); // show the new value
            }
            Err(e) => self.note(true, format!("✗ {} failed: {e}", c.command_line())),
        }
    }

    /// Show a write-mode notice for a while.
    pub fn note(&mut self, err: bool, s: impl Into<String>) {
        self.notice = Some((Instant::now(), s.into(), err));
    }

    /// Seconds since the last kstat sample, if the sampler looks stuck.
    pub fn stale_for(&self) -> Option<Duration> {
        let last = self.last_kstat?;
        let age = last.elapsed();
        (age > self.interval * 3 + Duration::from_secs(2)).then_some(age)
    }

    // ---------------------------------------------------------------- derived lists

    pub fn topo_rows(&self) -> Vec<TopoRow> {
        let mut out = Vec::new();
        for p in self.model.pools() {
            fn walk(
                app: &App,
                pool: &str,
                v: &Vdev,
                depth: usize,
                guides: &[bool],
                last: bool,
                out: &mut Vec<TopoRow>,
            ) {
                let key = vkey(pool, &v.name);
                let collapsed = app.topo_collapsed.contains(&key);
                out.push(TopoRow {
                    key,
                    pool: pool.to_string(),
                    name: v.name.clone(),
                    depth,
                    vtype: v.vtype.clone(),
                    has_children: !v.children.is_empty(),
                    collapsed,
                    guides: guides.to_vec(),
                    last,
                });
                if collapsed {
                    return;
                }
                let mut g = guides.to_vec();
                if depth > 0 {
                    g.push(!last);
                }
                let kids = app.sorted_children(pool, v);
                let n = kids.len();
                for (i, c) in kids.into_iter().enumerate() {
                    walk(app, pool, c, depth + 1, &g, i + 1 == n, out);
                }
            }
            walk(self, &p.name, &p.root, 0, &[], true, &mut out);
        }
        out
    }

    /// Sort value of a vdev for the current topology sort.
    pub fn topo_value(&self, pool: &str, v: &Vdev) -> f64 {
        let io = self.model.io(pool, &v.name);
        let f = |g: fn(&crate::parse::iostat::Row) -> f64| io.map(g).unwrap_or(-1.0);
        match self.topo_sort {
            TopoSort::Order | TopoSort::Name | TopoSort::State => 0.0,
            TopoSort::Errors => (v.errors() + v.slow_ios) as f64,
            TopoSort::Rops => f(|r| r.rops),
            TopoSort::Wops => f(|r| r.wops),
            TopoSort::Rbw => f(|r| r.rbw),
            TopoSort::Wbw => f(|r| r.wbw),
            TopoSort::Wait(i) => io.and_then(|r| r.wait[i]).unwrap_or(-1.0),
            TopoSort::Active => f(|r| r.active() as f64),
            TopoSort::Pending => f(|r| r.pending() as f64),
            TopoSort::Queue(i) => io.map(|r| (r.queue[i * 2] + r.queue[i * 2 + 1]) as f64).unwrap_or(-1.0),
            TopoSort::Size => v.size as f64,
            TopoSort::Alloc => v.alloc as f64,
            TopoSort::Free => v.size.saturating_sub(v.alloc) as f64,
            TopoSort::Cap => {
                if v.size > 0 {
                    v.alloc as f64 / v.size as f64
                } else {
                    -1.0
                }
            }
            TopoSort::Frag => v.frag.map(|x| x as f64).unwrap_or(-1.0),
        }
    }

    fn sorted_children<'a>(&self, pool: &str, v: &'a Vdev) -> Vec<&'a Vdev> {
        let mut kids: Vec<&Vdev> = v.children.iter().collect();
        if self.topo_sort == TopoSort::Order {
            return kids;
        }
        let desc = self.topo_desc;
        // class sections (special, logs, ...) keep their place after the data vdevs
        kids.sort_by(|a, b| {
            let (ca, cb) = (a.vtype == "class", b.vtype == "class");
            if ca || cb {
                return ca.cmp(&cb);
            }
            let o = match self.topo_sort {
                TopoSort::Name => a.name.cmp(&b.name),
                TopoSort::State => a.state.cmp(&b.state),
                _ => self.topo_value(pool, a).total_cmp(&self.topo_value(pool, b)),
            };
            if desc { o.reverse() } else { o }
        });
        kids
    }

    pub fn ds_rows(&self) -> Vec<DsRow> {
        let ds = self.model.datasets();
        let f = self.filter.to_lowercase();
        if self.ds_flat || !f.is_empty() {
            let mut v: Vec<&crate::parse::zjson::Dataset> =
                ds.iter().filter(|d| f.is_empty() || d.name.to_lowercase().contains(&f)).collect();
            self.sort_siblings(&mut v);
            return v
                .into_iter()
                .map(|d| DsRow { name: d.name.clone(), depth: 0, has_children: false, collapsed: false })
                .collect();
        }
        // Tree: datasets are sorted by name, so children follow their parent. Siblings are reordered by the sort key.
        let names: Vec<&str> = ds.iter().map(|d| d.name.as_str()).collect();
        let children = |parent: &str| -> Vec<&crate::parse::zjson::Dataset> {
            let prefix = format!("{parent}/");
            ds.iter().filter(|d| d.name.starts_with(&prefix) && !d.name[prefix.len()..].contains('/')).collect()
        };
        let mut out = Vec::new();
        fn rec<'a>(
            app: &App,
            d: &'a crate::parse::zjson::Dataset,
            depth: usize,
            children: &dyn Fn(&str) -> Vec<&'a crate::parse::zjson::Dataset>,
            out: &mut Vec<DsRow>,
        ) {
            let mut kids = children(&d.name);
            app.sort_siblings(&mut kids);
            let collapsed = app.ds_collapsed.contains(&d.name);
            out.push(DsRow { name: d.name.clone(), depth, has_children: !kids.is_empty(), collapsed });
            if !collapsed {
                for k in kids {
                    rec(app, k, depth + 1, children, out);
                }
            }
        }
        let mut roots: Vec<&crate::parse::zjson::Dataset> = ds
            .iter()
            .filter(|d| match d.name.rsplit_once('/') {
                None => true,
                Some((parent, _)) => !names.contains(&parent),
            })
            .collect();
        self.sort_siblings(&mut roots);
        for r in roots {
            rec(self, r, 0, &children, &mut out);
        }
        out
    }

    /// Sort value of a dataset for the current sort key (Name is handled separately).
    pub fn ds_value(&self, d: &crate::parse::zjson::Dataset) -> f64 {
        let p = &d.props;
        let io = self.model.ds_io.get(&d.name);
        let iov = |f: fn(&crate::model::DsIo) -> f64| io.map(f).unwrap_or(0.0);
        match self.ds_sort {
            DsSort::Name => 0.0,
            DsSort::Used => p.n("used") as f64,
            DsSort::Avail => p.n("available") as f64,
            DsSort::Refer => p.n("referenced") as f64,
            DsSort::Snap => p.n("usedbysnapshots") as f64,
            DsSort::Lused => p.n("logicalused") as f64,
            DsSort::Ratio => p.str("compressratio").trim_end_matches('x').parse().unwrap_or(1.0),
            DsSort::Quota => {
                let f = |a: u64, b: u64| if b > 0 { a as f64 / b as f64 } else { 0.0 };
                f(p.n("referenced"), p.n("refquota")).max(f(p.n("used"), p.n("quota")))
            }
            DsSort::Read => iov(|i| i.rbw),
            DsSort::Write => iov(|i| i.wbw),
            DsSort::Rops => iov(|i| i.rops),
            DsSort::Wops => iov(|i| i.wops),
            DsSort::Unlinks => iov(|i| i.unlinks),
            DsSort::ZilCommits => iov(|i| i.zil_commits),
            DsSort::ZilBytes => iov(|i| i.zil_bytes()),
        }
    }

    fn sort_siblings(&self, v: &mut [&crate::parse::zjson::Dataset]) {
        let desc = self.ds_desc;
        if self.ds_sort == DsSort::Name {
            v.sort_by(|a, b| if desc { b.name.cmp(&a.name) } else { a.name.cmp(&b.name) });
            return;
        }
        v.sort_by(|a, b| {
            let (x, y) = (self.ds_value(a), self.ds_value(b));
            let o = if desc { y.total_cmp(&x) } else { x.total_cmp(&y) };
            o.then_with(|| a.name.cmp(&b.name))
        });
    }

    /// Click on a sortable header: same column toggles direction, a new one starts at its natural direction.
    pub fn sort_datasets_by(&mut self, k: DsSort) {
        if self.ds_sort == k {
            self.ds_desc = !self.ds_desc;
        } else {
            self.ds_sort = k;
            self.ds_desc = k.default_desc();
        }
    }

    fn toggle_ds_fold(&mut self, name: &str) {
        if !self.ds_collapsed.remove(name) {
            self.ds_collapsed.insert(name.to_string());
        }
    }

    /// Sort value of a disk for the current disk sort (text sorts handled in `disk_rows`).
    pub fn disk_value(&self, d: &crate::collect::DiskInfo) -> f64 {
        let smart = self.model.smart.get(&d.smart_dev).and_then(|r| r.as_ref().ok());
        let rate = self.model.disk_rates.get(&d.whole);
        let io = self.model.io(&d.pool, &d.vdev);
        let o = |x: Option<u64>| x.map(|v| v as f64).unwrap_or(-1.0);
        match self.disk_sort {
            DiskSort::Size => d.size as f64,
            DiskSort::Temp => o(smart.and_then(|s| s.temp_c)),
            DiskSort::Hours => o(smart.and_then(|s| s.power_on_hours)),
            DiskSort::Defects => o(smart.and_then(|s| s.defects)),
            DiskSort::Errors => o(smart.and_then(|s| s.uncorrected)),
            DiskSort::Util => rate.map(|r| r.util).unwrap_or(-1.0),
            DiskSort::Await => rate.map(|r| r.r_await.max(r.w_await)).unwrap_or(-1.0),
            DiskSort::Rops => io.map(|r| r.rops).or(rate.map(|r| r.rops)).unwrap_or(-1.0),
            DiskSort::Wops => io.map(|r| r.wops).or(rate.map(|r| r.wops)).unwrap_or(-1.0),
            DiskSort::Wait => io
                .map(|r| {
                    r.wait[crate::parse::iostat::w::DISK_R]
                        .unwrap_or(0.0)
                        .max(r.wait[crate::parse::iostat::w::DISK_W].unwrap_or(0.0))
                })
                .unwrap_or(-1.0),
            _ => 0.0,
        }
    }

    /// Disk rows (filtered and sorted); pool order unless a header was clicked.
    pub fn disk_rows(&self) -> Vec<usize> {
        let f = self.filter.to_lowercase();
        let mut v: Vec<usize> = self
            .model
            .disks
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                f.is_empty()
                    || d.vdev.to_lowercase().contains(&f)
                    || d.whole.contains(&f)
                    || d.parent.to_lowercase().contains(&f)
                    || d.model().to_lowercase().contains(&f)
                    || d.slot().is_some_and(|s| s.to_lowercase().contains(&f))
                    || self
                        .model
                        .smart
                        .get(&d.smart_dev)
                        .and_then(|r| r.as_ref().ok())
                        .and_then(|s| s.serial.as_deref())
                        .is_some_and(|s| s.to_lowercase().contains(&f))
            })
            .map(|(i, _)| i)
            .collect();
        let disks = &self.model.disks;
        let desc = self.disk_desc;
        let text = |i: usize| -> Option<String> {
            let d = &disks[i];
            match self.disk_sort {
                DiskSort::Order => None,
                DiskSort::Slot => Some(d.slot().map(|s| s.to_string()).unwrap_or_else(|| format!("~{}", d.vdev))),
                DiskSort::Vdev => Some(d.vdev.clone()),
                DiskSort::Dev => Some(d.whole.clone()),
                DiskSort::Model => Some(d.model()),
                _ => None,
            }
        };
        match self.disk_sort {
            DiskSort::Order => {}
            DiskSort::Slot | DiskSort::Vdev | DiskSort::Dev | DiskSort::Model => v.sort_by(|&a, &b| {
                let o = natural(&text(a).unwrap_or_default(), &text(b).unwrap_or_default());
                if desc { o.reverse() } else { o }
            }),
            _ => v.sort_by(|&a, &b| {
                let o = self.disk_value(&disks[a]).total_cmp(&self.disk_value(&disks[b]));
                (if desc { o.reverse() } else { o }).then(a.cmp(&b))
            }),
        }
        v
    }

    /// First click sorts ascending, second reverses, third returns to ZFS's own property order.
    pub fn sort_props_by(&mut self, k: PropSort) {
        self.props_relocate = true;
        if self.props_sort == k && k != PropSort::Order {
            if !self.props_desc {
                self.props_desc = true;
            } else {
                self.props_sort = PropSort::Order;
                self.props_desc = false;
            }
        } else {
            self.props_sort = k;
            self.props_desc = false;
        }
    }

    pub fn sort_disks_by(&mut self, k: DiskSort) {
        if self.disk_sort == k {
            if self.disk_desc == k.default_desc() {
                self.disk_desc = !self.disk_desc;
            } else {
                self.disk_sort = DiskSort::Order;
                self.disk_desc = false;
            }
        } else {
            self.disk_sort = k;
            self.disk_desc = k.default_desc();
        }
    }

    /// Targets for the property browser: pools, then filesystems/volumes.
    pub fn prop_targets(&self) -> Vec<String> {
        // each pool, followed by its datasets (sorted by name, so children follow their parent)
        let mut v = Vec::new();
        for p in self.model.pools() {
            v.push(format!("pool:{}", p.name));
            v.extend(
                self.model
                    .datasets()
                    .iter()
                    .filter(|d| d.kind != DsKind::Snapshot && d.pool == p.name)
                    .map(|d| d.name.clone()),
            );
        }
        v
    }

    /// Pools & datasets column, filtered.
    pub fn prop_targets_shown(&self) -> Vec<String> {
        let f = self.target_filter.to_lowercase();
        self.prop_targets().into_iter().filter(|t| f.is_empty() || t.to_lowercase().contains(&f)).collect()
    }

    /// Set Props-tab focus (keeps `about_focus`, which the about pane reads, in step).
    pub fn set_props_focus(&mut self, f: PropsFocus) {
        self.props_focus = f;
        self.about_focus = f == PropsFocus::About;
    }

    fn select_target(&mut self, i: usize) {
        let t = self.prop_targets_shown();
        if let Some(name) = t.get(i) {
            if self.props_target.as_ref() != Some(name) {
                self.props_target = Some(name.clone());
                self.props_relocate = true; // stay on the same property, to compare it across datasets
            }
            self.picker_sel = i;
        }
    }

    pub fn props_target(&self) -> Option<String> {
        self.props_target.clone().or_else(|| self.model.pools().first().map(|p| format!("pool:{}", p.name)))
    }

    // ---------------------------------------------------------------- input

    pub fn handle_key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
            self.quit = true;
            return;
        }
        if self.modal.is_some() {
            self.modal_key(k);
            return;
        }
        if self.editing {
            let buf = if self.filter_targets { &mut self.target_filter } else { &mut self.filter };
            match k.code {
                KeyCode::Esc => {
                    self.editing = false;
                    buf.clear();
                }
                KeyCode::Enter => self.editing = false,
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            if self.filter_targets {
                self.picker_sel = 0;
                self.select_target(0);
            } else {
                self.after_filter_change();
            }
            return;
        }
        if self.show_help {
            match k.code {
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => self.help_scroll += 10,
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                _ => self.show_help = false,
            }
            return;
        }
        if self.tab == Tab::Datasets && self.ds_detail && matches!(k.code, KeyCode::Tab | KeyCode::BackTab) {
            // list → pane → (Properties pane only) about → list
            match (self.ds_focus_pane, self.about_focus) {
                (false, _) => self.ds_focus_pane = true,
                (true, false) if self.ds_pane == DsPane::Props => self.about_focus = true,
                _ => {
                    self.ds_focus_pane = false;
                    self.about_focus = false;
                }
            }
            return;
        }
        if self.tab == Tab::Props && matches!(k.code, KeyCode::Tab | KeyCode::BackTab) {
            let order = [PropsFocus::Targets, PropsFocus::Props, PropsFocus::About];
            let i = order.iter().position(|f| *f == self.props_focus).unwrap_or(0);
            let n = if k.code == KeyCode::Tab { (i + 1) % 3 } else { (i + 2) % 3 };
            self.set_props_focus(order[n]);
            return;
        }
        // the about pane has focus: movement keys scroll it
        let about_active = self.about_focus
            && (self.tab == Tab::Props
                || (self.tab == Tab::Datasets && self.ds_detail && self.ds_pane == DsPane::Props));
        if about_active && !self.editing {
            match k.code {
                KeyCode::Down | KeyCode::Char('j') => self.about_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => self.about_scroll = self.about_scroll.saturating_sub(1),
                KeyCode::PageDown => self.about_scroll += 10,
                KeyCode::PageUp => self.about_scroll = self.about_scroll.saturating_sub(10),
                KeyCode::Home | KeyCode::Char('g') => self.about_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.about_scroll = usize::MAX / 2,
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                    if self.tab == Tab::Props {
                        self.set_props_focus(PropsFocus::Props);
                    } else {
                        self.about_focus = false;
                    }
                }
                _ => {
                    if self.tab == Tab::Props {
                        self.set_props_focus(PropsFocus::Props);
                    } else {
                        self.about_focus = false;
                    }
                    self.handle_key(k);
                }
            }
            return;
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => {
                self.show_help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char(c @ '1'..='7') => self.set_tab(Tab::ALL[(c as u8 - b'1') as usize]),
            KeyCode::Tab => self.set_tab(Tab::ALL[(self.tab.idx() + 1) % Tab::ALL.len()]),
            KeyCode::BackTab => self.set_tab(Tab::ALL[(self.tab.idx() + Tab::ALL.len() - 1) % Tab::ALL.len()]),
            KeyCode::Char('p') => self.toggle_pause(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.step_interval(1),
            KeyCode::Char('-') => self.step_interval(-1),
            KeyCode::Char('b') => crate::fmt::set_exact(!crate::fmt::exact()),
            KeyCode::Char('r') => self.actions.push(Action::Refresh),
            KeyCode::Char('l') => self.show_log = !self.show_log,
            KeyCode::Char('W') => self.toggle_write(),
            KeyCode::Char('/') => {
                self.editing = true;
                self.filter_targets = self.tab == Tab::Props && self.props_focus == PropsFocus::Targets;
                if self.filter_targets {
                    self.target_filter.clear();
                } else {
                    self.filter.clear();
                }
            }
            KeyCode::Esc | KeyCode::Backspace => self.back(),
            _ => self.tab_key(k),
        }
    }

    /// Pause freezes the screen and releases the mouse so text (serials, models, paths) can be selected and
    /// copied with the terminal's own selection; resume re-baselines rates and captures the mouse again.
    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if !self.paused {
            if let Some(s) = self.held_kstats.take() {
                self.model.on_kstats(s, false);
            }
            for m in std::mem::take(&mut self.held) {
                self.on_msg(m);
            }
        }
        self.actions.push(Action::MouseCapture(!self.paused));
    }

    fn set_tab(&mut self, t: Tab) {
        if self.tab != t {
            self.tab = t;
            self.about_focus = false;
            self.param_sel = 0;
            self.filter.clear();
            self.scroll = 0;
        }
    }

    fn back(&mut self) {
        if self.tab == Tab::Props {
            match self.props_focus {
                PropsFocus::About => self.set_props_focus(PropsFocus::Props),
                PropsFocus::Props if !self.filter.is_empty() => self.filter.clear(),
                PropsFocus::Props => self.set_props_focus(PropsFocus::Targets),
                PropsFocus::Targets => {
                    self.target_filter.clear();
                    let cur = self.props_target();
                    self.picker_sel =
                        self.prop_targets_shown().iter().position(|t| Some(t) == cur.as_ref()).unwrap_or(0);
                }
            }
            return;
        }
        if self.about_focus {
            self.about_focus = false;
        } else if self.tab == Tab::Datasets && self.ds_detail && self.ds_focus_pane {
            self.ds_focus_pane = false;
        } else if !self.filter.is_empty() {
            self.filter.clear();
        } else if self.ds_detail && self.tab == Tab::Datasets {
            self.ds_detail = false;
        } else if self.disk_detail && self.tab == Tab::Disks {
            self.disk_detail = false;
        } else if self.pool_history && self.tab == Tab::Pool {
            self.pool_history = false;
        } else if self.grid && self.tab == Tab::Pool {
            self.grid = false;
        }
    }

    fn step_interval(&mut self, dir: i32) {
        let cur = self.interval.as_secs();
        let i = INTERVALS.iter().position(|&s| s >= cur).unwrap_or(0) as i32;
        let n = (i + dir).clamp(0, INTERVALS.len() as i32 - 1) as usize;
        let d = Duration::from_secs(INTERVALS[n]);
        if d != self.interval {
            self.interval = d;
            self.actions.push(Action::SetInterval(d));
        }
    }

    fn after_filter_change(&mut self) {
        self.scroll = 0;
        match self.tab {
            Tab::Datasets => {
                let rows = self.ds_rows();
                if !rows.iter().any(|r| Some(&r.name) == self.ds_sel.as_ref()) {
                    self.ds_sel = rows.first().map(|r| r.name.clone());
                }
            }
            Tab::Disks => self.disk_sel = 0,
            Tab::Props => self.props_sel = 0,
            Tab::Pool => self.hist_sel = 0,
            _ => {}
        }
    }

    /// Move a selection in a list of `n` rows.
    fn nav(k: KeyCode, cur: usize, n: usize, page: usize) -> Option<usize> {
        if n == 0 {
            return None;
        }
        let last = n - 1;
        Some(match k {
            KeyCode::Up | KeyCode::Char('k') => cur.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => (cur + 1).min(last),
            KeyCode::PageUp => cur.saturating_sub(page),
            KeyCode::PageDown => (cur + page).min(last),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => last,
            _ => return None,
        })
    }

    fn tab_key(&mut self, k: KeyEvent) {
        match self.tab {
            Tab::Pool => self.pool_key(k),
            Tab::Datasets => self.ds_key(k),
            Tab::Disks => self.disk_key(k),
            Tab::Props => self.props_key(k),
            Tab::Arc | Tab::Zil => {
                if k.code == KeyCode::Char('n') {
                    let n = self.model.pools().len().max(1);
                    self.txg_pool = (self.txg_pool + 1) % n;
                } else if let Some(i) = Self::nav(k.code, self.param_sel, 1000, 5) {
                    self.param_sel = i;
                }
            }
            _ => {}
        }
    }

    /// Pool the Pool tab is looking at (the selected row's pool).
    pub fn current_pool(&self) -> Option<String> {
        self.topo_sel
            .as_ref()
            .and_then(|k| k.split_once(':').map(|(p, _)| p.to_string()))
            .or_else(|| self.model.pools().first().map(|p| p.name.clone()))
    }

    fn pool_key(&mut self, k: KeyEvent) {
        if k.code == KeyCode::Char('H') {
            self.pool_history = !self.pool_history;
            self.hist_sel = 0;
            if self.pool_history
                && let Some(p) = self.current_pool()
            {
                self.actions.push(Action::History(p));
            }
            return;
        }
        if self.pool_history {
            if let Some(i) = Self::nav(k.code, self.hist_sel, 1_000_000, 20) {
                self.hist_sel = i;
            }
            return;
        }
        match k.code {
            KeyCode::Char('m') => self.io_mode = self.io_mode.next(),
            KeyCode::Char('d') => self.grid = !self.grid,
            KeyCode::Char('c') => self.grid_metric = self.grid_metric.next(),
            KeyCode::Char('w') => self.heat_disk = !self.heat_disk,
            // the grid hides the topology tree: arrows pick the metric rather than moving/folding unseen rows
            code if self.grid => {
                if let Some(i) = Self::nav(code, self.grid_metric.index(), GridMetric::ALL.len(), 1) {
                    self.grid_metric = GridMetric::ALL[i];
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(s) = self.topo_sel.clone() {
                    self.topo_collapsed.insert(s);
                }
            }
            KeyCode::Right | KeyCode::Enter => {
                if let Some(s) = &self.topo_sel
                    && !self.topo_collapsed.remove(s)
                    && k.code == KeyCode::Enter
                {
                    // Enter on an expanded row with children collapses it
                    let rows = self.topo_rows();
                    if rows.iter().any(|r| &r.key == s && r.has_children) {
                        self.topo_collapsed.insert(s.clone());
                    }
                }
            }
            KeyCode::Char(' ') => {
                if let Some(s) = self.topo_sel.clone()
                    && !self.topo_collapsed.remove(&s)
                {
                    self.topo_collapsed.insert(s);
                }
            }
            KeyCode::Char('e') => self.topo_collapsed.clear(),
            code => {
                let rows = self.topo_rows();
                let cur = rows.iter().position(|r| Some(&r.key) == self.topo_sel.as_ref()).unwrap_or(0);
                if let Some(i) = Self::nav(code, cur, rows.len(), 10) {
                    self.topo_sel = Some(rows[i].key.clone());
                }
            }
        }
    }

    fn ds_key(&mut self, k: KeyEvent) {
        if self.ds_detail {
            match k.code {
                KeyCode::Char('[') => {
                    let i = DsPane::ALL.iter().position(|p| *p == self.ds_pane).unwrap_or(0);
                    self.set_pane(DsPane::ALL[(i + DsPane::ALL.len() - 1) % DsPane::ALL.len()]);
                    return;
                }
                KeyCode::Char(']') => {
                    let i = DsPane::ALL.iter().position(|p| *p == self.ds_pane).unwrap_or(0);
                    self.set_pane(DsPane::ALL[(i + 1) % DsPane::ALL.len()]);
                    return;
                }
                KeyCode::Char('P') => {
                    if let Some(s) = &self.ds_sel {
                        self.props_target = Some(s.clone());
                        self.props_relocate = true;
                        self.target_filter.clear();
                        self.picker_sel = self.prop_targets().iter().position(|t| t == s).unwrap_or(0);
                        self.set_tab(Tab::Props);
                        self.set_props_focus(PropsFocus::Props);
                    }
                    return;
                }
                KeyCode::Char('s') if self.ds_focus_pane && self.ds_pane == DsPane::Props => {
                    self.cycle_prop_sort(false);
                    return;
                }
                KeyCode::Char('S') if self.ds_focus_pane && self.ds_pane == DsPane::Props => {
                    self.cycle_prop_sort(true);
                    return;
                }
                KeyCode::Char('e' | 'i' | 'a')
                    if self.can_write && self.ds_focus_pane && self.ds_pane == DsPane::Props =>
                {
                    self.write_key(k);
                    return;
                }
                // focused pane: the usual movement keys move its highlight
                code if self.ds_focus_pane => {
                    if let Some(i) = Self::nav(code, self.snap_sel, 1_000_000, 10) {
                        self.snap_sel = i;
                    }
                    return;
                }
                // unfocused: J/K and PgUp/PgDn still scroll the pane; ↑↓ move the tree selection
                KeyCode::Char('J') | KeyCode::PageDown => {
                    self.snap_sel = self.snap_sel.saturating_add(if k.code == KeyCode::PageDown { 10 } else { 1 });
                    return;
                }
                KeyCode::Char('K') | KeyCode::PageUp => {
                    self.snap_sel = self.snap_sel.saturating_sub(if k.code == KeyCode::PageUp { 10 } else { 1 });
                    return;
                }
                _ => {}
            }
        }
        match k.code {
            KeyCode::Char('t') => {
                self.ds_flat = !self.ds_flat;
                self.ds_sort = if self.ds_flat { DsSort::Read } else { DsSort::Name };
                self.ds_desc = self.ds_sort.default_desc();
            }
            KeyCode::Char('s') => {
                self.ds_sort = self.ds_sort.next(self.ds_flat);
                self.ds_desc = self.ds_sort.default_desc();
            }
            KeyCode::Char('S') => self.ds_desc = !self.ds_desc,
            KeyCode::Enter => {
                self.ds_detail = !self.ds_detail;
                self.ds_focus_pane = false;
                if self.ds_detail {
                    self.snap_sel = 0;
                    if self.ds_pane == DsPane::Snapshots {
                        self.request_snapshots();
                    }
                }
            }
            KeyCode::Char(' ') => {
                if let Some(s) = self.ds_sel.clone() {
                    self.toggle_ds_fold(&s);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(s) = self.ds_sel.clone() {
                    if self.ds_collapsed.contains(&s) || !self.ds_rows().iter().any(|r| r.name == s && r.has_children) {
                        // already collapsed / leaf: go to parent
                        if let Some((p, _)) = s.rsplit_once('/') {
                            self.ds_sel = Some(p.to_string());
                        }
                    } else {
                        self.ds_collapsed.insert(s);
                    }
                }
            }
            KeyCode::Right => {
                if let Some(s) = &self.ds_sel {
                    self.ds_collapsed.remove(s);
                }
            }
            KeyCode::Char('e') => self.ds_collapsed.clear(),
            KeyCode::Char('E') => {
                // collapse everything below depth 1
                for d in self.model.datasets() {
                    if d.name.matches('/').count() >= 1 {
                        self.ds_collapsed.insert(d.name.clone());
                    }
                }
            }
            code => {
                let rows = self.ds_rows();
                let cur = rows.iter().position(|r| Some(&r.name) == self.ds_sel.as_ref()).unwrap_or(0);
                if let Some(i) = Self::nav(code, cur, rows.len(), 10) {
                    self.ds_sel = Some(rows[i].name.clone());
                    if self.ds_pane == DsPane::Props {
                        self.props_relocate = true; // same property, next dataset
                    } else {
                        self.snap_sel = 0;
                    }
                    if self.ds_detail && self.ds_pane == DsPane::Snapshots {
                        self.request_snapshots();
                    }
                }
            }
        }
    }

    fn set_pane(&mut self, p: DsPane) {
        self.ds_pane = p;
        self.about_focus = false;
        self.snap_sel = 0;
        if p == DsPane::Snapshots {
            self.request_snapshots();
        }
    }

    pub fn request_snapshots(&mut self) {
        if let Some(s) = &self.ds_sel {
            self.actions.push(Action::Snapshots(s.clone()));
        }
    }

    fn disk_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('J') | KeyCode::PageDown if self.disk_detail => self.disk_scroll += 5,
            KeyCode::Char('K') | KeyCode::PageUp if self.disk_detail => {
                self.disk_scroll = self.disk_scroll.saturating_sub(5)
            }
            KeyCode::Char('s') => {
                const ORDER: [DiskSort; 8] = [
                    DiskSort::Order,
                    DiskSort::Util,
                    DiskSort::Wait,
                    DiskSort::Temp,
                    DiskSort::Errors,
                    DiskSort::Defects,
                    DiskSort::Hours,
                    DiskSort::Slot,
                ];
                let i = ORDER.iter().position(|x| *x == self.disk_sort).map(|i| i + 1).unwrap_or(0);
                self.disk_sort = ORDER[i % ORDER.len()];
                self.disk_desc = self.disk_sort.default_desc();
            }
            KeyCode::Char('S') => self.disk_desc = !self.disk_desc,
            KeyCode::Enter => {
                self.disk_detail = !self.disk_detail;
                self.disk_scroll = 0;
                if self.disk_detail
                    && let Some(d) = self.disk_rows().get(self.disk_sel).and_then(|&i| self.model.disks.get(i))
                {
                    self.actions.push(Action::Smart(d.smart_dev.clone()));
                }
            }
            code => {
                let n = self.disk_rows().len();
                if let Some(i) = Self::nav(code, self.disk_sel, n, 10) {
                    if i != self.disk_sel {
                        self.disk_scroll = 0;
                        if self.disk_detail
                            && let Some(d) = self.disk_rows().get(i).and_then(|&j| self.model.disks.get(j))
                        {
                            self.actions.push(Action::Smart(d.smart_dev.clone()));
                        }
                    }
                    self.disk_sel = i;
                }
            }
        }
    }

    fn cycle_prop_sort(&mut self, reverse: bool) {
        self.props_relocate = true;
        if reverse {
            self.props_desc = !self.props_desc;
        } else {
            self.props_sort = match self.props_sort {
                PropSort::Order => PropSort::Name,
                PropSort::Name => PropSort::Value,
                PropSort::Value => PropSort::Source,
                PropSort::Source => PropSort::Order,
            };
            self.props_desc = false;
        }
    }

    fn props_key(&mut self, k: KeyEvent) {
        match self.props_focus {
            PropsFocus::Targets => match k.code {
                KeyCode::Char('a') if self.can_write => {
                    self.write_key(k);
                }
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                    self.select_target(self.picker_sel);
                    self.set_props_focus(PropsFocus::Props);
                }
                code => {
                    let n = self.prop_targets_shown().len();
                    if let Some(i) = Self::nav(code, self.picker_sel, n, 10) {
                        self.select_target(i); // the property list follows as you move
                    }
                }
            },
            PropsFocus::Props => match k.code {
                KeyCode::Enter | KeyCode::Right => self.set_props_focus(PropsFocus::About),
                KeyCode::Left => self.set_props_focus(PropsFocus::Targets),
                KeyCode::Char('s') => self.cycle_prop_sort(false),
                KeyCode::Char('S') => self.cycle_prop_sort(true),
                KeyCode::Char('L') => {
                    self.props_local = !self.props_local;
                    self.props_relocate = true;
                }
                KeyCode::Char('e' | 'i' | 'a') if self.can_write => {
                    self.write_key(k);
                }
                code => {
                    if let Some(i) = Self::nav(code, self.props_sel, 100_000, 10) {
                        self.props_sel = i;
                    }
                }
            },
            PropsFocus::About => {}
        }
    }

    pub fn handle_mouse(&mut self, m: MouseEvent) {
        if self.modal.is_some() {
            return; // dialogs are keyboard-only
        }
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                // the wheel scrolls whatever is under the pointer
                let under = self.hits.at(m.column, m.row);
                self.about_focus = matches!(under, Some(Hit::AboutArea));
                if self.tab == Tab::Props {
                    self.set_props_focus(match under {
                        Some(Hit::AboutArea) => PropsFocus::About,
                        Some(Hit::TargetRow(_)) => PropsFocus::Targets,
                        Some(_) => PropsFocus::Props,
                        None => self.props_focus,
                    });
                }
                if self.tab == Tab::Datasets && self.ds_detail {
                    self.ds_focus_pane = matches!(under, Some(Hit::PaneRow(_) | Hit::PaneArea | Hit::AboutArea));
                }
                let code = if m.kind == MouseEventKind::ScrollDown { KeyCode::Down } else { KeyCode::Up };
                self.handle_key(KeyEvent::from(code));
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let double = self
                    .last_click
                    .is_some_and(|(t, x, y)| t.elapsed() < Duration::from_millis(400) && x == m.column && y == m.row);
                self.last_click = Some((Instant::now(), m.column, m.row));
                match self.hits.at(m.column, m.row) {
                    Some(Hit::Tab(t)) => self.set_tab(t),
                    Some(Hit::DsSort(k)) => self.sort_datasets_by(k),
                    Some(Hit::DiskSort(k)) => self.sort_disks_by(k),
                    Some(Hit::PropSort(k)) => self.sort_props_by(k),
                    Some(Hit::TopoSort(k)) => {
                        if self.topo_sort == k {
                            // third click on a column returns to pool order
                            if self.topo_desc == k.default_desc() {
                                self.topo_desc = !self.topo_desc;
                            } else {
                                self.topo_sort = TopoSort::Order;
                                self.topo_desc = false;
                            }
                        } else {
                            self.topo_sort = k;
                            self.topo_desc = k.default_desc();
                        }
                    }
                    Some(Hit::Fold(i)) => {
                        self.select_row(i);
                        match self.tab {
                            Tab::Datasets => {
                                if let Some(s) = self.ds_sel.clone() {
                                    self.toggle_ds_fold(&s);
                                }
                            }
                            Tab::Pool => {
                                if let Some(s) = self.topo_sel.clone()
                                    && !self.topo_collapsed.remove(&s)
                                {
                                    self.topo_collapsed.insert(s);
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Hit::SubTab(i)) => {
                        if let Some(p) = DsPane::ALL.get(i) {
                            self.set_pane(*p);
                        }
                    }
                    Some(Hit::PaneRow(i)) => {
                        self.ds_focus_pane = true;
                        self.about_focus = false;
                        self.snap_sel = i;
                    }
                    Some(Hit::PaneArea) => self.ds_focus_pane = true,
                    Some(Hit::GridMetric(g)) => self.grid_metric = g,
                    Some(Hit::TargetRow(i)) => {
                        self.set_props_focus(PropsFocus::Targets);
                        self.select_target(i);
                        if double {
                            self.set_props_focus(PropsFocus::Props);
                        }
                    }
                    Some(Hit::AboutArea) => {
                        if self.tab == Tab::Props {
                            self.set_props_focus(PropsFocus::About);
                        }
                        self.about_focus = true;
                        if self.tab == Tab::Datasets {
                            self.ds_focus_pane = true;
                        }
                    }
                    Some(Hit::Row(i)) => {
                        self.about_focus = false;
                        if self.tab == Tab::Props {
                            self.set_props_focus(PropsFocus::Props);
                        }
                        if self.tab == Tab::Datasets {
                            self.ds_focus_pane = false;
                        }
                        self.select_row(i);
                        if double {
                            self.handle_key(KeyEvent::from(KeyCode::Enter));
                        }
                    }
                    None => {}
                }
            }
            MouseEventKind::Down(MouseButton::Right) => self.back(),
            _ => {}
        }
    }

    // ---------------------------------------------------------------- write mode

    /// `W`: unlock (after a prompt) or lock write mode.
    pub fn toggle_write(&mut self) {
        if !self.can_write {
            self.note(true, self.write_off_reason);
        } else if self.rw {
            self.rw = false;
            tracing::warn!("write mode locked");
            self.note(false, "write mode off: zfscope is read-only again");
        } else {
            self.modal = Some(Modal::Unlock);
        }
    }

    /// Pool or dataset whose properties the current view shows.
    pub fn prop_view_target(&self) -> Option<Target> {
        match self.tab {
            Tab::Props => self.props_target().map(|t| match t.strip_prefix("pool:") {
                Some(p) => Target::Pool(p.to_string()),
                None => Target::Dataset(t),
            }),
            Tab::Datasets if self.ds_detail && self.ds_pane == DsPane::Props => {
                self.ds_sel.clone().map(Target::Dataset)
            }
            _ => None,
        }
    }

    pub fn target_props(&self, t: &Target) -> Option<&Props> {
        match t {
            Target::Pool(p) => self.model.pool_props(p),
            Target::Dataset(d) => self.model.dataset(d).map(|d| &d.props),
        }
    }

    /// `e` set / `i` inherit / `a` add in a property view.
    fn write_key(&mut self, k: KeyEvent) {
        let KeyCode::Char(c) = k.code else { return };
        if !self.rw {
            self.note(false, "write mode is off: W unlocks it");
            return;
        }
        if self.write_busy {
            self.note(true, "a change is still running");
            return;
        }
        let Some(target) = self.prop_view_target() else { return };
        if c == 'a' {
            self.modal = Some(Modal::Edit(EditState {
                target,
                key: String::new(),
                input: String::new(),
                orig: String::new(),
                opt_sel: None,
                naming: true,
                new_prop: true,
                error: None,
            }));
            return;
        }
        let Some(key) = self.props_sel_key.clone() else {
            self.note(true, "no property selected");
            return;
        };
        let Some(p) = self.target_props(&target).and_then(|ps| ps.get(&key)).cloned() else {
            self.note(true, format!("{key}: not found on {}", target.name()));
            return;
        };
        if c == 'i' {
            let why = match (&target, p.source.as_str()) {
                (Target::Pool(_), _) => "pool properties aren't inherited: set the value you want with e".into(),
                (_, "LOCAL") => String::new(),
                (_, "INHERITED") => format!("{key} is already inherited from {}", p.from),
                (_, "DEFAULT") => format!("{key} is already at its default"),
                (_, "RECEIVED") => format!("{key} was received; zfscope doesn't revert received values"),
                _ => format!("{key} can't be inherited"),
            };
            if !why.is_empty() {
                self.note(true, why);
                return;
            }
            let dataset = target.name().to_string();
            self.modal = Some(Modal::Confirm(ConfirmState {
                change: Change::Inherit { dataset, key },
                old: Some(p),
                typed: String::new(),
                error: None,
            }));
            return;
        }
        if crate::ui::props::read_only(&p) {
            self.note(true, format!("{key} is read-only (a statistic, or fixed when it was created)"));
            return;
        }
        let input = if key.starts_with("feature@") {
            if p.value != "disabled" {
                self.note(true, format!("{key} is already {}; feature flags can't be disabled", p.value));
                return;
            }
            "enabled".to_string()
        } else if p.num == Some(0) && matches!(key.as_str(), "quota" | "refquota" | "reservation" | "refreservation") {
            "none".to_string()
        } else {
            p.value.clone()
        };
        let orig = if key.starts_with("feature@") { p.value.clone() } else { input.clone() };
        self.modal = Some(Modal::Edit(EditState {
            target,
            key,
            input,
            orig,
            opt_sel: None,
            naming: false,
            new_prop: false,
            error: None,
        }));
    }

    fn modal_key(&mut self, k: KeyEvent) {
        let Some(modal) = self.modal.take() else { return };
        self.modal = match modal {
            Modal::Unlock => {
                if matches!(k.code, KeyCode::Char('y' | 'Y')) {
                    self.rw = true;
                    tracing::warn!("write mode unlocked");
                    self.note(false, "write mode on: e set · i inherit · a add, in the property views");
                }
                None
            }
            Modal::Edit(e) => self.edit_key(e, k),
            Modal::Confirm(c) => self.confirm_key(c, k),
        };
    }

    fn edit_key(&mut self, mut e: EditState, k: KeyEvent) -> Option<Modal> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return None,
            KeyCode::Char('u') if ctrl => {
                e.input.clear();
                e.opt_sel = None;
            }
            KeyCode::Char(c) if !ctrl => {
                e.input.push(c);
                e.opt_sel = None;
                e.error = None;
            }
            KeyCode::Backspace => {
                e.input.pop();
                e.opt_sel = None;
                e.error = None;
            }
            KeyCode::Up | KeyCode::Down => {
                let n = e.options().len();
                if n > 0 {
                    let i = match (e.opt_sel, k.code) {
                        (None, KeyCode::Down) => 0,
                        (None, _) => n - 1,
                        (Some(i), KeyCode::Down) => (i + 1) % n,
                        (Some(i), _) => (i + n - 1) % n,
                    };
                    e.opt_sel = Some(i);
                    e.input = option_fill(e.options()[i].0);
                    e.error = None;
                }
            }
            KeyCode::Enter if e.naming => {
                let name = e.input.trim().to_string();
                if let Err(msg) = crate::write::valid_user_key(&name) {
                    e.error = Some(msg);
                    return Some(Modal::Edit(e));
                }
                // an existing user property: edit its value instead
                let cur = self.target_props(&e.target).and_then(|p| p.get(&name)).map(|p| p.value.clone());
                e.naming = false;
                e.key = name;
                e.new_prop = cur.is_none();
                e.orig = cur.clone().unwrap_or_default();
                e.input = cur.unwrap_or_default();
                e.error = None;
            }
            KeyCode::Enter => {
                if e.input == e.orig {
                    self.note(false, format!("{} unchanged", e.key));
                    return None;
                }
                let change = Change::Set { target: e.target.clone(), key: e.key.clone(), value: e.input.clone() };
                if let Err(msg) = change.validate() {
                    e.error = Some(msg);
                    return Some(Modal::Edit(e));
                }
                let old = self.target_props(&e.target).and_then(|p| p.get(&e.key)).cloned();
                return Some(Modal::Confirm(ConfirmState { change, old, typed: String::new(), error: None }));
            }
            _ => {}
        }
        Some(Modal::Edit(e))
    }

    fn confirm_key(&mut self, mut c: ConfirmState, k: KeyEvent) -> Option<Modal> {
        if c.change.one_way() {
            match k.code {
                KeyCode::Esc => {
                    self.note(false, "cancelled");
                    return None;
                }
                KeyCode::Backspace => {
                    c.typed.pop();
                }
                KeyCode::Char(ch) => c.typed.push(ch),
                KeyCode::Enter if c.typed == c.change.target().pool() => {
                    self.run_change(c);
                    return None;
                }
                KeyCode::Enter => c.error = Some("type the pool name exactly to confirm (Esc cancels)".into()),
                _ => {}
            }
            return Some(Modal::Confirm(c));
        }
        if matches!(k.code, KeyCode::Char('y' | 'Y')) {
            self.run_change(c);
        } else {
            self.note(false, "cancelled");
        }
        None
    }

    fn run_change(&mut self, c: ConfirmState) {
        let was = match &c.old {
            Some(p) => format!("was {:?} ({})", p.value, p.source.to_lowercase()),
            None => "new property".into(),
        };
        tracing::warn!("write: confirmed `{}` ({was})", c.change.command_line());
        self.write_busy = true;
        self.actions.push(Action::Write(c.change));
    }

    fn select_row(&mut self, i: usize) {
        match self.tab {
            Tab::Pool => {
                if let Some(r) = self.topo_rows().get(i) {
                    self.topo_sel = Some(r.key.clone());
                }
            }
            Tab::Datasets => {
                if let Some(r) = self.ds_rows().get(i) {
                    self.ds_sel = Some(r.name.clone());
                    if self.ds_detail && self.ds_pane == DsPane::Snapshots {
                        self.request_snapshots();
                    }
                }
            }
            Tab::Disks => self.disk_sel = i,
            Tab::Props => self.props_sel = i,
            _ => {}
        }
    }
}
