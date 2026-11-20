//! Block-device data: `/proc/diskstats`, `smartctl -j -a` (SCSI/SAS, SATA and NVMe), `/etc/zfs/vdev_id.conf`.

use std::collections::HashMap;

use serde_json::Value;

/// Cumulative counters for one block device (`/proc/diskstats`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DiskCounters {
    pub reads: u64,
    pub read_sectors: u64,
    pub read_ms: u64,
    pub writes: u64,
    pub write_sectors: u64,
    pub write_ms: u64,
    pub in_flight: u64,
    pub io_ms: u64,
    pub weighted_ms: u64,
}

pub fn parse_diskstats(text: &str) -> HashMap<String, DiskCounters> {
    text.lines()
        .filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            if t.len() < 14 {
                return None;
            }
            let n = |i: usize| t[i].parse::<u64>().unwrap_or(0);
            Some((
                t[2].to_string(),
                DiskCounters {
                    reads: n(3),
                    read_sectors: n(5),
                    read_ms: n(6),
                    writes: n(7),
                    write_sectors: n(9),
                    write_ms: n(10),
                    in_flight: n(11),
                    io_ms: n(12),
                    weighted_ms: n(13),
                },
            ))
        })
        .collect()
}

/// The SMART facts the Disks view shows. Everything is optional: SAS, SATA and NVMe report different things.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Smart {
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub passed: Option<bool>,
    pub temp_c: Option<u64>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub rotation_rpm: Option<u64>,
    pub form_factor: Option<String>,
    /// SAS grown defect list / SATA reallocated sectors
    pub defects: Option<u64>,
    /// SAS uncorrected read+write+verify errors / SATA pending+offline-uncorrectable / NVMe media errors
    pub uncorrected: Option<u64>,
    /// NVMe percentage used (endurance) / SSD life used
    pub wear_pct: Option<u64>,
    pub nvme_spare_pct: Option<u64>,
    pub nvme_unsafe_shutdowns: Option<u64>,
    /// Terabytes read/written over the device's life.
    pub read_tb: Option<f64>,
    pub written_tb: Option<f64>,
    /// smartctl exit status bits (bit 3 = disk failing, bits 4-7 = prefail/usage/error-log/self-test history)
    pub exit_status: u64,
    /// Every interesting key/value pair, for the detail view.
    pub details: Vec<(String, String)>,
}

fn gb_str(v: &Value) -> Option<f64> {
    v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64())
}

pub fn parse_smart(text: &str) -> Result<Smart, String> {
    let d: Value = serde_json::from_str(text).map_err(|e| format!("smartctl: bad JSON: {e}"))?;
    let mut s = Smart {
        exit_status: d["smartctl"]["exit_status"].as_u64().unwrap_or(0),
        model: d["model_name"].as_str().or(d["product"].as_str()).map(|s| s.trim().to_string()),
        serial: d["serial_number"].as_str().map(str::to_string),
        firmware: d["firmware_version"].as_str().or(d["revision"].as_str()).map(str::to_string),
        passed: d["smart_status"]["passed"].as_bool(),
        temp_c: d["temperature"]["current"].as_u64(),
        power_on_hours: d["power_on_time"]["hours"].as_u64(),
        power_cycles: d["power_cycle_count"].as_u64(),
        rotation_rpm: d["rotation_rate"].as_u64(),
        form_factor: d["form_factor"]["name"].as_str().map(str::to_string),
        ..Default::default()
    };
    if s.model.is_none()
        && d["device"].is_null()
        && let Some(msg) = d["smartctl"]["messages"][0]["string"].as_str()
    {
        return Err(format!("smartctl: {msg}"));
    }
    let mut det = Vec::new();
    // SAS / SCSI
    if let Some(g) = d["scsi_grown_defect_list"].as_u64() {
        s.defects = Some(g);
        det.push(("grown defects".into(), g.to_string()));
    }
    let log = &d["scsi_error_counter_log"];
    if log.is_object() {
        let mut unc = 0;
        for k in ["read", "write", "verify"] {
            let e = &log[k];
            if let Some(u) = e["total_uncorrected_errors"].as_u64() {
                unc += u;
                det.push((format!("{k} uncorrected"), u.to_string()));
            }
            if let Some(c) = e["total_errors_corrected"].as_u64() {
                det.push((format!("{k} corrected"), c.to_string()));
            }
        }
        s.uncorrected = Some(unc);
        s.read_tb = gb_str(&log["read"]["gigabytes_processed"]).map(|g| g / 1000.0);
        s.written_tb = gb_str(&log["write"]["gigabytes_processed"]).map(|g| g / 1000.0);
    }
    if let Some(p) = d["scsi_percentage_used_endurance_indicator"].as_u64() {
        s.wear_pct = Some(p);
    }
    // NVMe
    let n = &d["nvme_smart_health_information_log"];
    if n.is_object() {
        s.uncorrected = n["media_errors"].as_u64();
        s.wear_pct = n["percentage_used"].as_u64();
        s.nvme_spare_pct = n["available_spare"].as_u64();
        s.nvme_unsafe_shutdowns = n["unsafe_shutdowns"].as_u64();
        s.temp_c = s.temp_c.or(n["temperature"].as_u64());
        s.power_on_hours = s.power_on_hours.or(n["power_on_hours"].as_u64());
        // data units are 1000 * 512 bytes
        s.read_tb = n["data_units_read"].as_u64().map(|u| u as f64 * 512_000.0 / 1e12);
        s.written_tb = n["data_units_written"].as_u64().map(|u| u as f64 * 512_000.0 / 1e12);
        if let Some(m) = n.as_object() {
            for (k, v) in m {
                if !v.is_object() && !v.is_array() {
                    det.push((k.replace('_', " "), v.to_string()));
                }
            }
        }
    }
    // ATA
    if let Some(tab) = d["ata_smart_attributes"]["table"].as_array() {
        let raw = |id: u64| tab.iter().find(|a| a["id"].as_u64() == Some(id)).and_then(|a| a["raw"]["value"].as_u64());
        s.defects = raw(5);
        let pending = raw(197).unwrap_or(0) + raw(198).unwrap_or(0);
        s.uncorrected = Some(pending);
        for a in tab {
            det.push((
                format!("{:>3} {}", a["id"].as_u64().unwrap_or(0), a["name"].as_str().unwrap_or("")),
                a["raw"]["string"].as_str().map(str::to_string).unwrap_or_else(|| a["raw"]["value"].to_string()),
            ));
        }
    }
    if s.exit_status & 0b1111_1000 != 0 {
        det.push(("smartctl exit status".into(), format!("{:#b}", s.exit_status)));
    }
    s.details = det;
    Ok(s)
}

/// `/etc/zfs/vdev_id.conf` aliases: alias -> (device path, trailing comment).
pub fn parse_vdev_id(text: &str) -> HashMap<String, (String, String)> {
    text.lines()
        .filter_map(|l| {
            let (body, comment) = l.split_once('#').unwrap_or((l, ""));
            let mut t = body.split_whitespace();
            (t.next()? == "alias").then_some(())?;
            let (alias, dev) = (t.next()?, t.next()?);
            Some((alias.to_string(), (dev.to_string(), comment.trim().to_string())))
        })
        .collect()
}

/// `enclosure_device:SLOT 00,PPHKT9SP` (sysfs link name) -> "SLOT 00".
pub fn enclosure_slot(link_name: &str) -> Option<String> {
    let rest = link_name.strip_prefix("enclosure_device:")?;
    let slot = rest.split(',').next()?.trim();
    (!slot.is_empty()).then(|| slot.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diskstats() {
        let d = parse_diskstats(
            "   8       0 sda 29142242 0 2362455476 234999422 21815326 0 18907636840 654495562 0 320105181 889494987 0 0 0 0 1 3\n",
        );
        let s = d["sda"];
        assert_eq!(s.reads, 29142242);
        assert_eq!(s.write_sectors, 18907636840);
        assert_eq!(s.io_ms, 320105181);
    }

    #[test]
    fn smart_sas() {
        let s = parse_smart(r#"{"smartctl":{"exit_status":0},"device":{"type":"scsi"},"model_name":"WDC WUH722020CL5204","revision":"CDE0","serial_number":"X1","smart_status":{"passed":true},"temperature":{"current":30},"power_on_time":{"hours":1250},"rotation_rate":7200,"scsi_grown_defect_list":2,
            "scsi_error_counter_log":{"read":{"total_errors_corrected":0,"gigabytes_processed":"1270.872","total_uncorrected_errors":1},"write":{"total_uncorrected_errors":0,"gigabytes_processed":"9680.692"},"verify":{"total_uncorrected_errors":0}}}"#).unwrap();
        assert_eq!(s.model.as_deref(), Some("WDC WUH722020CL5204"));
        assert_eq!(s.firmware.as_deref(), Some("CDE0"));
        assert_eq!((s.temp_c, s.power_on_hours, s.defects, s.uncorrected), (Some(30), Some(1250), Some(2), Some(1)));
        assert!((s.written_tb.unwrap() - 9.680692).abs() < 1e-6);
    }

    #[test]
    fn smart_nvme() {
        let s = parse_smart(r#"{"smartctl":{"exit_status":0},"device":{"type":"nvme"},"model_name":"Dell Ent NVMe","serial_number":"S","firmware_version":"2.0.5","smart_status":{"passed":true},
            "nvme_smart_health_information_log":{"temperature":39,"available_spare":100,"percentage_used":3,"data_units_read":2000000,"media_errors":0,"power_on_hours":1157,"unsafe_shutdowns":14}}"#).unwrap();
        assert_eq!((s.temp_c, s.wear_pct, s.uncorrected, s.power_on_hours), (Some(39), Some(3), Some(0), Some(1157)));
        assert!((s.read_tb.unwrap() - 1.024).abs() < 1e-9);
    }

    #[test]
    fn vdev_id() {
        let m = parse_vdev_id(
            "# comment\nalias e00s00  /dev/disk/by-id/wwn-0x5000cca0e758b458   # SN PPHKT9SP\nmultipath no\n",
        );
        assert_eq!(m["e00s00"], ("/dev/disk/by-id/wwn-0x5000cca0e758b458".into(), "SN PPHKT9SP".into()));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn slot() {
        assert_eq!(enclosure_slot("enclosure_device:SLOT 00,PPHKT9SP             ").as_deref(), Some("SLOT 00"));
        assert_eq!(enclosure_slot("power"), None);
    }
}
