#!/usr/bin/env python3
"""Runs ON the ZFS host (as root, via `ssh host python3 -`); writes a gzipped tar to stdout.

Captures everything zfscope reads, in the layout FixtureSource replays:

  meta.json                         host, zfs version, ticks, interval
  frames/NNN/proc/...               per-tick kstats, /proc/diskstats, /proc/meminfo
  iostat.txt                        raw `zpool iostat -Hpvlq -y 1 N` stream
  hist/NNN-w.txt, hist/NNN-r.txt    cumulative latency / request-size histograms (every HIST_EVERY ticks)
  cmd/*.json                        zpool status/list/get, zfs list/get (JSON)
  root/...                          static files: vdev_id.conf, modprobe.d zfs options, sysfs disk attributes, by-vdev links, zfs params
  smart/<dev>.json                  smartctl -j -a

Dataset/snapshot names are data on these hosts: every dataset path component and snapshot name is replaced by a
pseudonym (ds01, snap01, ...) here on the server, before anything is written, so real names never leave the host.
Mountpoints are mapped the same way; keylocation, comments and user-property values are redacted.
"""

import io
import json
import os
import re
import socket
import subprocess
import sys
import tarfile
import threading
import time

TICKS = int(os.environ.get("TICKS", "60"))
INTERVAL = 1.0
HIST_EVERY = int(os.environ.get("HIST_EVERY", "5"))
SMART = os.environ.get("SMART", "1") == "1"
KSTAT = "/proc/spl/kstat/zfs"
GLOBAL_KSTATS = ["arcstats", "zil", "dmu_tx", "zfetchstats", "dbufstats", "abdstats", "dnodestats", "vdev_mirror_stats"]
POOL_KSTATS = ["iostats", "txgs", "state", "dmu_tx_assign"]

files = {}  # tar path -> bytes
links = {}  # tar path -> link target


def run(cmd, timeout=120):
    try:
        return subprocess.run(cmd, capture_output=True, timeout=timeout).stdout
    except Exception:
        return b""


def read(path):
    try:
        with open(path, "rb") as f:
            return f.read()
    except Exception:
        return None


# ---------------------------------------------------------------- pseudonyms
names = run(["zfs", "list", "-H", "-p", "-o", "name", "-t", "all"]).decode().split()
pools = run(["zpool", "list", "-H", "-o", "name"]).decode().split()
comp_map, snap_map = {}, {}


def comp(c):
    if c not in comp_map:
        comp_map[c] = "ds%02d" % (len(comp_map) + 1)
    return comp_map[c]


def snap(s):
    if s not in snap_map:
        snap_map[s] = "snap%02d" % (len(snap_map) + 1)
    return snap_map[s]


full_map = {p: p for p in pools}
for n in sorted(names):
    base, sep, tail = re.match(r"([^@#]*)([@#]?)(.*)", n).groups()
    parts = base.split("/")
    out = [parts[0]] + [comp(p) for p in parts[1:]]
    pseudo = "/".join(out)
    if sep:
        pseudo += sep + snap(tail)
    full_map[n] = pseudo

_full_re = None
if full_map:
    alts = sorted((k for k in full_map if k not in pools), key=len, reverse=True)
    if alts:
        _full_re = re.compile(r"(?<![A-Za-z0-9_.\-])(" + "|".join(re.escape(a) for a in alts) + r")(?![A-Za-z0-9_.\-])")


def scrub_text(s):
    if _full_re is None:
        return s
    return _full_re.sub(lambda m: full_map[m.group(1)], s)


def scrub_path(p):
    """Map each component of a filesystem path; unknown components get their own pseudonym too."""
    if not p.startswith("/"):
        return p
    done = set(comp_map.values())
    return "/".join(x if x in ("", *pools) or x in done else comp(x) for x in p.split("/"))


REDACT_PROPS = {"keylocation": "file:///redacted/key", "comment": "redacted"}


def scrub_props(obj):
    """Structured scrub of zfs/zpool -j property maps."""
    if isinstance(obj, dict):
        props = obj.get("properties")
        if isinstance(props, dict):
            for k, v in props.items():
                if not isinstance(v, dict) or not isinstance(v.get("value"), str):
                    continue
                val = v["value"]
                if k in REDACT_PROPS and val not in ("-", "none", "prompt", "", "default"):
                    v["value"] = REDACT_PROPS[k]
                elif k == "mountpoint":
                    v["value"] = scrub_path(val)
                elif ":" in k and k not in ("com.sun:auto-snapshot",):
                    if val not in ("-", "on", "off", "true", "false") and not val.isdigit():
                        v["value"] = "redacted"
        for v in obj.values():
            scrub_props(v)
    elif isinstance(obj, list):
        for v in obj:
            scrub_props(v)
    return obj


def put(path, data):
    if data is None:
        return
    if isinstance(data, str):
        data = data.encode()
    files[path] = data


def put_text(path, data):
    if data is None:
        return
    put(path, scrub_text(data.decode(errors="replace")))


def put_json(path, raw):
    try:
        obj = json.loads(scrub_text(raw.decode(errors="replace")))
    except Exception:
        put(path, raw)
        return
    put(path, json.dumps(scrub_props(obj), indent=1))


# Pool history mentions datasets that no longer exist (so they aren't in full_map): map every pool-prefixed name
# component-wise, every absolute path component-wise, and redact key locations and user-property values.
_name_re = re.compile(r"(?<![\w.\-/])(" + "|".join(re.escape(p) for p in pools) + r")((?:/[^\s@#'\",()\[\]=]+)*)([@#][^\s'\",()\[\]]+)?") if pools else None
_path_re = re.compile(r"(?<![\w.:/])(/(?!dev/|sys/|proc/|etc/zfs)[^\s'\",]+)")


def scrub_history(text):
    def rep(m):
        out = m.group(1)
        if m.group(2):
            out += "".join("/" + comp(x) for x in m.group(2).split("/")[1:])
        if m.group(3):
            out += m.group(3)[0] + snap(m.group(3)[1:])
        return out
    if _name_re is not None:
        text = _name_re.sub(rep, text)
    text = _path_re.sub(lambda m: scrub_path(m.group(1)), text)
    text = re.sub(r"keylocation=\S+", "keylocation=file:///redacted/key", text)
    text = re.sub(r"(\b[\w.-]+:[\w.-]+)=(\S+)", lambda m: m.group(1) + "=redacted", text)
    text = re.sub(r"^(\s+[\w.-]+:[\w.-]+): .*$", lambda m: m.group(1) + ": redacted", text, flags=re.M)
    return text


# ---------------------------------------------------------------- static data
put_json("cmd/zpool-status.json", run(["zpool", "status", "-j", "--json-int"]))
put_json("cmd/zpool-list.json", run(["zpool", "list", "-v", "-j", "--json-int"]))
put_json("cmd/zpool-get.json", run(["zpool", "get", "-j", "-p", "all"]))
put_json("cmd/zfs-version.json", run(["zfs", "version", "-j"]))
put("root/etc/zfs/vdev_id.conf", read("/etc/zfs/vdev_id.conf"))
mdir = "/etc/modprobe.d"
for f in sorted(os.listdir(mdir)) if os.path.isdir(mdir) else []:
    if f.endswith(".conf"):
        text = read(mdir + "/" + f)
        if text and re.search(rb"^\s*options\s+zfs\s", text, flags=re.M):
            put("root" + mdir + "/" + f, text)
for pool in pools:
    h = run(["zpool", "history", "-il", pool]).decode(errors="replace").splitlines()
    put("cmd/zpool-history-%s.txt" % pool, scrub_history("\n".join(h[:1] + h[-8000:]) + "\n"))

pdir = "/sys/module/zfs/parameters"
for p in sorted(os.listdir(pdir)) if os.path.isdir(pdir) else []:
    put("root" + pdir + "/" + p, read(pdir + "/" + p))

# Leaf vdev paths -> block devices -> sysfs attributes (+ multipath legs, partitions' parents).
status = json.loads(files["cmd/zpool-status.json"] or b"{}")
leaf_paths = []


def walk(v):
    if not isinstance(v, dict):
        return
    if v.get("path"):
        leaf_paths.append(v["path"])
    for c in (v.get("vdevs") or {}).values():
        walk(c)


for pool in (status.get("pools") or {}).values():
    for key in ("vdevs", "special", "dedup", "logs", "l2cache", "spares"):
        for v in (pool.get(key) or {}).values():
            walk(v)

SYS_ATTRS = [
    "size", "dev", "ro", "removable", "partition", "dm/name", "dm/uuid",
    "queue/rotational", "queue/scheduler", "queue/logical_block_size", "queue/physical_block_size",
    "queue/nr_requests", "queue/max_sectors_kb", "queue/write_cache",
    "device/vendor", "device/model", "device/rev", "device/wwid", "device/serial", "device/firmware_rev",
    "device/state", "device/queue_depth", "device/timeout", "device/ioerr_cnt", "device/iotmo_cnt",
    "device/iodone_cnt", "device/iorequest_cnt", "device/sas_address", "device/numa_node",
    "device/device/model", "device/device/serial", "device/device/firmware_rev", "device/device/numa_node",
    "wwid", "nsid",
]
seen_dev = set()
smart_devs = []


def add_link(path):
    """Record a symlink (raw target) under root/, return resolved absolute path."""
    try:
        tgt = os.readlink(path)
    except OSError:
        return path
    links["root" + path] = tgt
    return os.path.normpath(os.path.join(os.path.dirname(path), tgt))


def add_dev(dev):
    if dev in seen_dev:
        return
    seen_dev.add(dev)
    base = "/sys/class/block/" + dev
    for a in SYS_ATTRS:
        put("root" + base + "/" + a, read(base + "/" + a))
    for sub in ("slaves", "holders"):
        d = base + "/" + sub
        for s in sorted(os.listdir(d)) if os.path.isdir(d) else []:
            links["root" + d + "/" + s] = "../../" + s
    dd = base + "/device"
    for e in sorted(os.listdir(dd)) if os.path.isdir(dd) else []:
        if e.startswith("enclosure_device:"):
            links["root" + dd + "/" + e] = "../enclosure"
    # partition -> parent: record the parent name explicitly (sysfs puts partitions inside the parent's dir)
    real = os.path.realpath(base)
    if os.path.exists(base + "/partition"):
        parent = os.path.basename(os.path.dirname(real))
        put("root" + base + "/parent", parent + "\n")
        add_dev(parent)
    for s in sorted(os.listdir(base + "/slaves")) if os.path.isdir(base + "/slaves") else []:
        add_dev(s)
    if not dev.startswith("dm-") and not os.path.exists(base + "/partition"):
        if not re.match(r"nvme\d+c\d+n\d+", dev):
            smart_devs.append(dev)


for p in leaf_paths:
    cur = p
    for _ in range(4):
        if not os.path.islink(cur):
            break
        cur = add_link(cur)
    # whole-disk vdevs are labelled with partition 1; zfs shows the disk path, IO goes to -part1
    for cand in (p + "-part1",):
        if os.path.islink(cand):
            add_dev(os.path.basename(add_link(cand)))
    add_dev(os.path.basename(cur))

if SMART:
    done_mpath = set()
    for d in smart_devs:
        base = "/sys/class/block/%s/holders" % d
        hs = tuple(sorted(os.listdir(base))) if os.path.isdir(base) else ()
        if hs and hs in done_mpath:  # one leg per multipath map is enough
            continue
        done_mpath.add(hs)
        put("smart/%s.json" % d, run(["smartctl", "-j", "-a", "/dev/" + d], timeout=30))

# ---------------------------------------------------------------- time series
iostat_out = []


def iostat_reader():
    p = subprocess.Popen(["zpool", "iostat", "-Hpvlq", "-y", "%d" % INTERVAL, str(TICKS)], stdout=subprocess.PIPE)
    iostat_out.append(p.stdout.read())
    p.wait()


th = threading.Thread(target=iostat_reader)
th.start()
t0 = time.monotonic()
for t in range(TICKS):
    fr = "frames/%03d" % t
    for k in GLOBAL_KSTATS:
        put_text(fr + KSTAT + "/" + k, read(KSTAT + "/" + k))
    for pool in pools:
        pd = KSTAT + "/" + pool
        for k in POOL_KSTATS:
            put_text(fr + pd + "/" + k, read(pd + "/" + k))
        for e in os.listdir(pd) if os.path.isdir(pd) else []:
            if e.startswith("objset-"):
                put_text(fr + pd + "/" + e, read(pd + "/" + e))
    put(fr + "/proc/diskstats", read("/proc/diskstats"))
    put(fr + "/proc/meminfo", read("/proc/meminfo"))
    if t % HIST_EVERY == 0:
        put("hist/%03d-w.txt" % t, run(["zpool", "iostat", "-Hpvw"]))
        put("hist/%03d-r.txt" % t, run(["zpool", "iostat", "-Hpvr"]))
    if t == TICKS // 2:  # space numbers move slowly; one mid-capture snapshot is plenty
        put_json("cmd/zfs-list.json", run(["zfs", "list", "-j", "--json-int", "-t", "all", "-o", "all"]))
    time.sleep(max(0.0, t0 + (t + 1) * INTERVAL - time.monotonic()))
th.join()
put("iostat.txt", iostat_out[0] if iostat_out else b"")

meta = {
    "host": socket.gethostname().split(".")[0],
    "ticks": TICKS,
    "interval": INTERVAL,
    "hist_every": HIST_EVERY,
    "captured": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
    "uname": os.uname().release,
    "pseudonyms": {"datasets": len(comp_map), "snapshots": len(snap_map)},
}
put("meta.json", json.dumps(meta, indent=1))

# ---------------------------------------------------------------- tar out
buf = sys.stdout.buffer
with tarfile.open(fileobj=buf, mode="w|gz") as tar:
    now = time.time()
    for path, data in sorted(files.items()):
        ti = tarfile.TarInfo(path)
        ti.size, ti.mtime, ti.mode = len(data), now, 0o644
        tar.addfile(ti, io.BytesIO(data))
    for path, tgt in sorted(links.items()):
        ti = tarfile.TarInfo(path)
        ti.type, ti.linkname, ti.mtime = tarfile.SYMTYPE, tgt, now
        tar.addfile(ti)
