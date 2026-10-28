//! Scheduled prompts: once, daily, weekly or cron, evaluated in the panel's
//! timezone (Asia/Tehran by default). Fired schedules enqueue a run; when the
//! target chat is busy the run simply waits in the queue.
use crate::chats::{create_chat, NewChat};
use crate::runner::{enqueue, NewRun};
use crate::settings;
use crate::state::St;
use crate::store::{get_chat, get_schedule, Schedule};
use crate::util::now;
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use rusqlite::params;
use serde_json::json;
use std::time::Duration;

fn parse_hm(s: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").map_err(|_| anyhow!("time must be HH:MM"))
}

fn local_to_utc(tz: Tz, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    // `earliest` handles DST gaps/overlaps (Tehran has no DST today, other zones might).
    tz.from_local_datetime(&naive).earliest().map(|d| d.with_timezone(&Utc))
}

/// Next fire time strictly after `after`.
pub fn next_after(s: &Schedule, tz: Tz, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    let local_after = after.with_timezone(&tz);
    match s.kind.as_str() {
        "once" => {
            let raw = s.run_at.as_deref().ok_or_else(|| anyhow!("date/time required"))?;
            let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
                .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M"))
                .map_err(|_| anyhow!("date/time must be YYYY-MM-DDTHH:MM"))?;
            let at = local_to_utc(tz, naive).ok_or_else(|| anyhow!("invalid local time"))?;
            Ok((at > after).then_some(at))
        }
        "daily" | "weekly" => {
            let t = parse_hm(s.time_of_day.as_deref().unwrap_or(""))?;
            let days: Vec<u32> = if s.kind == "weekly" {
                let d: Vec<u32> = s
                    .weekdays
                    .as_deref()
                    .unwrap_or("")
                    .split(',')
                    .filter_map(|x| x.trim().parse().ok())
                    .filter(|d| *d < 7)
                    .collect();
                if d.is_empty() {
                    bail!("pick at least one weekday");
                }
                d
            } else {
                (0..7).collect()
            };
            let start: NaiveDate = local_after.date_naive();
            for i in 0..=8 {
                let date = start + chrono::Duration::days(i);
                if !days.contains(&date.weekday().num_days_from_sunday()) {
                    continue;
                }
                if let Some(at) = local_to_utc(tz, date.and_time(t)) {
                    if at > after {
                        return Ok(Some(at));
                    }
                }
            }
            Ok(None)
        }
        "cron" => {
            let expr = s.cron.as_deref().unwrap_or("").trim();
            let cron = croner::Cron::new(expr).parse().map_err(|e| anyhow!("invalid cron: {e}"))?;
            let next = cron
                .find_next_occurrence(&local_after, false)
                .map_err(|e| anyhow!("cron: {e}"))?;
            Ok(Some(next.with_timezone(&Utc)))
        }
        k => bail!("unknown schedule kind {k}"),
    }
}

/// The next `n` fire times, formatted in local time (for the UI preview).
pub fn preview(s: &Schedule, tz: Tz, n: usize) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut after = Utc::now();
    for _ in 0..n {
        match next_after(s, tz, after)? {
            Some(t) => {
                out.push(t.with_timezone(&tz).format("%a %Y-%m-%d %H:%M").to_string());
                after = t;
            }
            None => break,
        }
    }
    Ok(out)
}

pub fn recompute(st: &St, id: i64) -> Result<()> {
    let s = get_schedule(&st.db, id)?;
    let next = if s.enabled { next_after(&s, settings::tz(&st.db), Utc::now())? } else { None };
    st.db.conn().execute(
        "UPDATE schedules SET next_run_at=?2 WHERE id=?1",
        params![id, next.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))],
    )?;
    Ok(())
}

pub fn recompute_all(st: &St) {
    let ids: Vec<i64> = {
        let c = st.db.conn();
        let mut q = match c.prepare("SELECT id FROM schedules") {
            Ok(q) => q,
            Err(_) => return,
        };
        q.query_map([], |r| r.get(0)).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default()
    };
    for id in ids {
        let _ = recompute(st, id);
    }
}

/// Fire a schedule now (also used by the "Run now" button).
pub fn fire(st: &St, id: i64) -> Result<i64> {
    let s = get_schedule(&st.db, id)?;
    let prompts: Vec<String> = serde_json::from_value(s.prompts.clone()).unwrap_or_default();
    let prompts: Vec<String> = prompts.into_iter().filter(|p| !p.trim().is_empty()).collect();
    let Some((first, rest)) = prompts.split_first() else { bail!("schedule has no prompt") };
    let tz = settings::tz(&st.db);
    let mut chat_id = None;
    if s.mode == "continue" {
        if let Some(cid) = s.chat_id {
            if get_chat(&st.db, cid).is_ok() {
                chat_id = Some(cid);
            }
        }
    }
    let chat_id = match chat_id {
        Some(c) => c,
        None => {
            let title = if s.mode == "continue" {
                format!("⏰ {}", s.name)
            } else {
                format!("⏰ {} · {}", s.name, Utc::now().with_timezone(&tz).format("%m-%d %H:%M"))
            };
            let chat = create_chat(
                st,
                NewChat {
                    project_id: s.project_id,
                    user_id: s.user_id,
                    title,
                    tool: s.tool.clone(),
                    model: s.model.clone(),
                    effort: s.effort.clone(),
                    permission_mode: s.permission_mode.clone(),
                    worktree: s.use_worktree,
                    auto_deploy: s.auto_deploy,
                    schedule_id: Some(s.id),
                    read_only: false,
                },
            )?;
            if s.mode == "continue" {
                st.db.conn().execute("UPDATE schedules SET chat_id=?2 WHERE id=?1", params![s.id, chat.id])?;
            }
            chat.id
        }
    };
    enqueue(
        st,
        NewRun {
            chat_id,
            user_id: s.user_id,
            source: "schedule",
            schedule_id: Some(s.id),
            prompt: first,
            attachments: json!([]),
            chain: rest.to_vec(),
            author: None,
        },
    )?;
    st.db.conn().execute("UPDATE schedules SET last_run_at=?2 WHERE id=?1", params![s.id, now()])?;
    Ok(chat_id)
}

pub fn spawn(st: St) {
    recompute_all(&st);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let due: Vec<(i64, String)> = {
                let c = st.db.conn();
                let mut q = match c.prepare(
                    "SELECT id, kind FROM schedules WHERE enabled=1 AND next_run_at IS NOT NULL AND next_run_at<=?1",
                ) {
                    Ok(q) => q,
                    Err(_) => continue,
                };
                q.query_map([now()], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map(|it| it.filter_map(Result::ok).collect())
                    .unwrap_or_default()
            };
            for (id, kind) in due {
                if let Err(e) = fire(&st, id) {
                    st.notice("error", format!("Schedule #{id} failed to start: {e:#}"));
                }
                if kind == "once" {
                    let _ = st.db.conn().execute("UPDATE schedules SET enabled=0, next_run_at=NULL WHERE id=?1", [id]);
                } else if let Err(e) = recompute(&st, id) {
                    st.notice("error", format!("Schedule #{id}: {e:#}"));
                    let _ = st.db.conn().execute("UPDATE schedules SET next_run_at=NULL WHERE id=?1", [id]);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(kind: &str) -> Schedule {
        Schedule {
            id: 1,
            project_id: 1,
            user_id: None,
            name: "t".into(),
            enabled: true,
            kind: kind.into(),
            run_at: None,
            time_of_day: None,
            weekdays: None,
            cron: None,
            tool: "claude".into(),
            model: String::new(),
            effort: String::new(),
            permission_mode: "bypass".into(),
            mode: "new".into(),
            chat_id: None,
            prompts: json!(["x"]),
            use_worktree: false,
            auto_deploy: false,
            last_run_at: None,
            next_run_at: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn daily_in_tehran() {
        let tz: Tz = "Asia/Tehran".parse().unwrap();
        let mut s = sched("daily");
        s.time_of_day = Some("05:00".into());
        // 2026-01-01 00:00 UTC = 03:30 Tehran → next is 05:00 Tehran = 01:30 UTC same day.
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let n = next_after(&s, tz, after).unwrap().unwrap();
        assert_eq!(n, Utc.with_ymd_and_hms(2026, 1, 1, 1, 30, 0).unwrap());
    }

    #[test]
    fn weekly_picks_next_weekday() {
        let tz: Tz = "UTC".parse().unwrap();
        let mut s = sched("weekly");
        s.time_of_day = Some("09:00".into());
        s.weekdays = Some("1".into()); // Monday
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap(); // Thursday
        let n = next_after(&s, tz, after).unwrap().unwrap();
        assert_eq!(n, Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap());
    }

    #[test]
    fn cron_every_15_minutes() {
        let tz: Tz = "UTC".parse().unwrap();
        let mut s = sched("cron");
        s.cron = Some("*/15 * * * *".into());
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 10, 7, 0).unwrap();
        let n = next_after(&s, tz, after).unwrap().unwrap();
        assert_eq!(n, Utc.with_ymd_and_hms(2026, 1, 1, 10, 15, 0).unwrap());
    }

    #[test]
    fn once_in_past_is_none() {
        let tz: Tz = "UTC".parse().unwrap();
        let mut s = sched("once");
        s.run_at = Some("2020-01-01T10:00".into());
        assert!(next_after(&s, tz, Utc::now()).unwrap().is_none());
    }
}
