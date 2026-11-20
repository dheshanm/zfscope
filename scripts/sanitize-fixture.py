#!/usr/bin/env python3
"""Turn a capture (scripts/capture-fixture.sh) into a committable demo fixture.

    scripts/sanitize-fixture.py tests/fixtures/local/<host> tests/fixtures/demo [--frames 40]

The capture already has dataset/snapshot names pseudonymised on the server. This additionally replaces everything
that identifies the real host, consistently across every file (and file names / symlink targets):

  * host names (and FQDNs)                    -> zfs-demo / zfs-demo.example.org
  * disk serial numbers                        -> ZD000001 ...
  * WWNs / SAS addresses (5000xxxxxxxxxxxx)    -> 5000c50000000001 ...
  * pool / vdev / dataset GUIDs                -> deterministic fake numbers
  * IPv4 addresses (NFS export lists, history) -> 192.0.2.x / 198.51.100.x (documentation ranges)
  * dataset pseudonyms ds01, ds02 ...          -> friendly words (home, projects, ...)

and trims the time series to --frames ticks (the fixture replays in a loop). It finishes with an audit that fails if
any original identifier is still present anywhere in the output.
"""

import argparse
import json
import os
import re
import shutil
import sys

WORDS = [
    "data", "home", "projects", "scratch", "backups", "archive", "vm", "db", "media", "shared", "research", "lab",
    "genomics", "imaging", "analysis", "raw", "processed", "results", "users", "alice", "bob", "carol", "dave", "erin",
    "frank", "grace", "heidi", "ivan", "judy", "mallory", "niaj", "olivia", "peggy", "rupert", "sybil", "trent",
    "victor", "walter", "build", "ci", "cache", "logs", "mail", "web", "git", "docker", "k8s", "postgres", "mysql",
    "redis", "minio", "nfs", "smb", "iscsi", "templates", "isos", "snapshots-old", "staging", "prod", "dev", "test",
    "qa", "reports", "finance", "hr", "legal", "marketing", "sales", "support", "ops", "infra", "monitoring",
    "grafana", "prometheus", "elastic", "kafka", "spark", "hadoop", "jupyter", "notebooks", "models", "datasets",
    "training", "inference", "exports", "imports", "incoming", "outgoing", "tmp", "spool", "queue", "mirror",
    "replica", "offsite", "cold", "warm", "hot", "tier1", "tier2", "tier3",
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--frames", type=int, default=20)
    ap.add_argument("--host", default="zfs-demo")
    a = ap.parse_args()

    meta = json.load(open(os.path.join(a.src, "meta.json")))
    real_host = meta["host"]
    files = {}  # rel path -> bytes
    links = {}  # rel path -> target
    for root, dirs, fs in os.walk(a.src):
        for f in fs + [d for d in dirs if os.path.islink(os.path.join(root, d))]:
            full = os.path.join(root, f)
            rel = os.path.relpath(full, a.src)
            if os.path.islink(full):
                links[rel] = os.readlink(full)
            else:
                files[rel] = open(full, "rb").read()

    # ------------------------------------------------------------------ trim the time series
    hist_every = int(meta.get("hist_every", 5))
    keep = lambda rel: not (
        (rel.startswith("frames/") and int(rel.split("/")[1]) >= a.frames)
        or (rel.startswith("hist/") and int(rel.split("/")[1].split("-")[0]) >= a.frames)
    )
    files = {k: v for k, v in files.items() if keep(k)}
    # iostat: first N blocks
    text = files["iostat.txt"].decode()
    first = text.split("\t", 1)[0]
    blocks, cur = [], []
    for line in text.splitlines():
        if line.split("\t", 1)[0] == first and cur:
            blocks.append(cur)
            cur = []
        cur.append(line)
    if cur:
        blocks.append(cur)
    files["iostat.txt"] = ("\n".join("\n".join(b) for b in blocks[: a.frames]) + "\n").encode()
    # diskstats: only devices the fixture knows about
    devs = {p.split("/")[4] for p in list(files) + list(links) if p.startswith("root/sys/class/block/")}
    for k in [k for k in files if k.endswith("proc/diskstats")]:
        lines = files[k].decode().splitlines()
        files[k] = ("\n".join(l for l in lines if len(l.split()) > 2 and l.split()[2] in devs) + "\n").encode()
    # sysfs: only the attributes zfscope reads; module parameters: only the ones it shows
    used_attrs = (
        "/size", "/partition", "/parent", "/dm/name", "/dm/uuid", "/wwid", "/queue/rotational", "/queue/scheduler",
        "/queue/logical_block_size", "/queue/physical_block_size", "/device/model", "/device/vendor", "/device/rev",
        "/device/wwid", "/device/state", "/device/ioerr_cnt", "/device/iotmo_cnt", "/device/sas_address",
        "/device/serial", "/device/firmware_rev",
    )
    help_src = open(os.path.join(os.path.dirname(__file__), "..", "src", "help.rs")).read()
    shown_params = set(re.findall(r'"((?:zfs|zil|zio|l2arc|spa|metaslab)_[a-z0-9_]+)"', help_src))
    for k in list(files):
        if k.startswith("root/sys/class/block/") and not k.endswith(used_attrs):
            del files[k]
        elif k.startswith("root/sys/module/zfs/parameters/") and k.rsplit("/", 1)[1] not in shown_params:
            del files[k]
    for k in list(links):
        if "/holders/" in k:
            del links[k]
    # command output: compact JSON, recent history only
    for k in [k for k in files if k.startswith("cmd/") and k.endswith(".json")]:
        try:
            files[k] = json.dumps(json.loads(files[k]), separators=(",", ":")).encode()
        except Exception:
            pass
    for k in [k for k in files if k.startswith("cmd/zpool-history-")]:
        lines = files[k].decode(errors="replace").splitlines()
        tail = lines[-2000:]
        while tail and not re.match(r"\d{4}-\d\d-\d\d\.", tail[0]):  # start at an entry boundary
            tail = tail[1:]
        files[k] = ("\n".join(lines[:1] + tail) + "\n").encode()
    # SMART JSON: drop unique device ids we don't use
    for k in [k for k in files if k.startswith("smart/")]:
        try:
            d = json.loads(files[k])
        except Exception:
            continue
        for key in ("wwn", "logical_unit_id", "nvme_ieee_oui_identifier", "local_time", "smartctl"):
            d.pop(key, None)
        files[k] = json.dumps(d, separators=(",", ":")).encode()

    # ------------------------------------------------------------------ collect identifiers
    blob = b"\n".join(files.values()).decode(errors="replace") + "\n".join(list(files) + list(links) + list(links.values()))
    serials = set()
    for k, v in files.items():
        if k.startswith("smart/"):
            try:
                s = json.loads(v).get("serial_number")
                if s:
                    serials.add(s)
            except Exception:
                pass
    serials |= set(re.findall(r"SN (\w{6,})", blob))
    serials |= set(re.findall(r"enclosure_device:SLOT \d+,(\w+)", blob))
    wwns = set(m.lower() for m in re.findall(r"(?i)5000[0-9a-f]{12}", blob))
    guids = set()
    for k, v in files.items():
        if k.endswith(".json"):
            try:
                walk_guids(json.loads(v), guids)
            except Exception:
                pass
    ips = set(re.findall(r"(?<![\d.])(?:\d{1,3}\.){3}\d{1,3}(?![\d.])", blob))
    ips = {i for i in ips if all(int(x) < 256 for x in i.split("."))}
    fqdns = set(re.findall(re.escape(real_host) + r"(?:\.[A-Za-z0-9-]+)+", blob))
    comps = sorted(set(re.findall(r"\bds(\d{2,3})\b", blob)), key=int)

    rep = {}
    for i, s in enumerate(sorted(serials)):
        rep[s] = "ZD%06d" % (i + 1)
    for i, w in enumerate(sorted(wwns)):
        rep[w] = "5000c5%010x" % (i + 1)
    for i, g in enumerate(sorted(guids)):
        rep[g] = str(1000000000000000000 + 7919 * (i + 1))
    for i, ip in enumerate(sorted(ips)):
        rep[ip] = ("192.0.2.%d" % (i + 1)) if i < 250 else ("198.51.100.%d" % (i - 249))
    for f in sorted(fqdns, key=len, reverse=True):
        rep[f] = a.host + ".example.org"
    rep[real_host] = a.host
    for i, c in enumerate(comps):
        rep["ds" + c] = WORDS[i] if i < len(WORDS) else "ds" + c
    # longest first, with boundaries so ds1 doesn't eat ds10 and hostnames don't eat substrings
    keys = sorted(rep, key=len, reverse=True)
    rx = re.compile(r"(?<![A-Za-z0-9])(" + "|".join(re.escape(k) for k in keys) + r")(?![A-Za-z0-9])", re.I)

    def sub(s):
        return rx.sub(lambda m: rep.get(m.group(1), rep.get(m.group(1).lower(), m.group(1))), s)

    # ------------------------------------------------------------------ write
    if os.path.exists(a.dst):
        shutil.rmtree(a.dst)
    for rel, data in files.items():
        out = os.path.join(a.dst, sub(rel))
        os.makedirs(os.path.dirname(out), exist_ok=True)
        try:
            data = sub(data.decode()).encode()
        except UnicodeDecodeError:
            pass
        if rel == "meta.json":
            m = json.loads(data)
            m["host"] = a.host
            m["ticks"] = a.frames
            m["sanitized"] = True
            m.pop("uname", None)
            data = json.dumps(m, indent=1).encode()
        open(out, "wb").write(data)
    for rel, tgt in links.items():
        out = os.path.join(a.dst, sub(rel))
        os.makedirs(os.path.dirname(out), exist_ok=True)
        os.symlink(sub(tgt), out)

    # ------------------------------------------------------------------ audit
    leaks = []
    for root, dirs, fs in os.walk(a.dst):
        for f in fs + dirs:
            full = os.path.join(root, f)
            hay = full + ("\n" + os.readlink(full) if os.path.islink(full) else "")
            if os.path.isfile(full) and not os.path.islink(full):
                hay += "\n" + open(full, "rb").read().decode(errors="replace")
            for tok in list(serials) + list(wwns) + list(ips) + [real_host] + list(guids):
                if tok and re.search(r"(?<![A-Za-z0-9])" + re.escape(tok) + r"(?![A-Za-z0-9])", hay, re.I):
                    leaks.append((os.path.relpath(full, a.dst), tok))
    if leaks:
        for f, t in leaks[:20]:
            print("LEAK %s: %s" % (f, t), file=sys.stderr)
        sys.exit(1)
    size = sum(os.lstat(os.path.join(r, f)).st_size for r, _, fs in os.walk(a.dst) for f in fs)
    print(
        "wrote %s: %d frames, %d serials, %d wwns, %d guids, %d ips, %d dataset names replaced, %.1f MB"
        % (a.dst, a.frames, len(serials), len(wwns), len(guids), len(ips), len(comps), size / 1e6)
    )


def walk_guids(o, out):
    if isinstance(o, dict):
        for k, v in o.items():
            if "guid" in k and isinstance(v, (int, str)) and str(v).isdigit() and len(str(v)) > 6:
                out.add(str(v))
            if isinstance(v, dict) and "guid" in k and str(v.get("value", "")).isdigit():
                out.add(str(v["value"]))
            walk_guids(v, out)
    elif isinstance(o, list):
        for v in o:
            walk_guids(v, out)


if __name__ == "__main__":
    main()
