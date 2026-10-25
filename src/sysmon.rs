//! Server CPU/RAM figures for the dashboard (read straight from /proc).
use crate::state::St;
use crate::util::meminfo;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub struct SysMon {
    cpu_percent: Mutex<f64>,
}

fn cpu_times() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/stat").ok()?;
    let line = s.lines().next()?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((v.iter().sum(), idle))
}

pub fn spawn(st: St) {
    tokio::spawn(async move {
        let mut prev = cpu_times();
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let cur = cpu_times();
            if let (Some((t0, i0)), Some((t1, i1))) = (prev, cur) {
                let dt = t1.saturating_sub(t0) as f64;
                if dt > 0.0 {
                    *st.sysmon.cpu_percent.lock().unwrap() = 100.0 * (1.0 - (i1.saturating_sub(i0) as f64 / dt));
                }
            }
            prev = cur;
        }
    });
}

fn process_rss_mb(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

pub fn snapshot(st: &St) -> Value {
    let m = meminfo();
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let load: Vec<&str> = load.split_whitespace().take(3).collect();
    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()))
        .unwrap_or(0.0);
    json!({
        "cpu_percent": (*st.sysmon.cpu_percent.lock().unwrap() * 10.0).round() / 10.0,
        "cpus": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        "load": load,
        "mem": m,
        "uptime_secs": uptime as u64,
        "app_rss_mb": process_rss_mb(std::process::id()),
        "claude_processes": st.runner.claude.lock().unwrap().len(),
    })
}
