//! Token usage and cost reports (the Usage page).
//!
//! Every finished run stores one `run_usage` row per model it used. A report reads
//! the rows in a date range (calendar days in the panel's timezone, weeks start on
//! Saturday) and sums them by day, project, user, model, tool, chat and schedule.
//! Rows without a reported cost (Codex) are priced from the price table when the
//! report is built, so price updates (daily, see `prices`) also update past estimates.
use crate::auth::AuthUser;
use crate::settings;
use crate::state::St;
use crate::util::{ApiError, ApiResult};
use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// USD per 1M tokens for models matching `model` (`*` is a wildcard).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Price {
    pub model: String,
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default)]
    pub output: f64,
    /// "auto" (kept up to date from the public price list) or "manual" (added or
    /// edited by an admin; never overwritten). Missing on tables saved before 0.6.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Price {
    /// Same model and prices, ignoring where the row came from.
    pub fn same_price(&self, o: &Price) -> bool {
        self.model.trim().eq_ignore_ascii_case(o.model.trim())
            && [self.input, self.cache_read, self.cache_write, self.output] == [o.input, o.cache_read, o.cache_write, o.output]
    }
}

pub fn parse_prices(s: &str) -> Result<Vec<Price>, String> {
    let list: Vec<Price> = serde_json::from_str(s).map_err(|e| e.to_string())?;
    for p in &list {
        if p.model.trim().is_empty() {
            return Err("every row needs a model pattern".into());
        }
        if [p.input, p.cache_read, p.cache_write, p.output].iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(format!("prices for {} must be zero or more", p.model));
        }
    }
    Ok(list)
}

fn prices(st: &St) -> Vec<Price> {
    let s = settings::get(&st.db, "usage_prices");
    parse_prices(&s)
        .ok()
        .filter(|l| !s.trim().is_empty() && !l.is_empty())
        .unwrap_or_else(|| parse_prices(settings::DEFAULT_PRICES).unwrap_or_default())
}

/// Case-insensitive match where `*` stands for any run of characters.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.trim().to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// The price for a model: a row with its exact name wins, otherwise the first row
/// whose pattern matches. A context suffix such as `[1m]` is ignored.
pub fn price_for<'a>(list: &'a [Price], model: &str) -> Option<&'a Price> {
    let model = model.trim();
    let model = match model.find('[') {
        Some(i) if model.ends_with(']') => &model[..i],
        _ => model,
    };
    list.iter()
        .find(|p| !p.model.contains('*') && p.model.trim().eq_ignore_ascii_case(model))
        .or_else(|| list.iter().find(|p| glob(&p.model, model)))
}

#[derive(Default, Clone, Serialize)]
pub struct Agg {
    pub runs: usize,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
    pub web_search: i64,
    pub total_tokens: i64,
    /// Reported plus estimated cost.
    pub cost: f64,
    /// The part of `cost` that is estimated from the price table.
    pub estimated_cost: f64,
    #[serde(skip)]
    run_ids: HashSet<i64>,
}

impl Agg {
    fn add(&mut self, r: &Row, cost: f64, estimated: bool) {
        self.run_ids.insert(r.run_id);
        self.runs = self.run_ids.len();
        self.input += r.input;
        self.output += r.output;
        self.cache_read += r.cache_read;
        self.cache_write += r.cache_write;
        self.reasoning += r.reasoning;
        self.web_search += r.web_search;
        self.total_tokens += r.input + r.output + r.cache_read + r.cache_write;
        self.cost += cost;
        if estimated {
            self.estimated_cost += cost;
        }
    }
}

struct Row {
    run_id: i64,
    chat_id: i64,
    project_id: i64,
    user_id: Option<i64>,
    author: Option<String>,
    schedule_id: Option<i64>,
    source: String,
    tool: String,
    model: String,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    web_search: i64,
    cost_usd: Option<f64>,
    legacy: bool,
    at: String,
}

#[derive(Deserialize, Default)]
pub struct Params {
    /// today | week | month | custom
    pub range: Option<String>,
    /// YYYY-MM-DD, inclusive, in the panel timezone (custom range).
    pub from: Option<String>,
    pub to: Option<String>,
    /// A user id, or `ext:<name>` for prompts sent from Telegram.
    pub user: Option<String>,
    pub project_id: Option<i64>,
    pub tool: Option<String>,
}

/// First and last calendar day (inclusive) of a range, in the panel timezone.
pub fn day_range(range: &str, today: NaiveDate, from: Option<&str>, to: Option<&str>) -> Result<(NaiveDate, NaiveDate), String> {
    let parse = |s: Option<&str>, what: &str| {
        NaiveDate::parse_from_str(s.unwrap_or("").trim(), "%Y-%m-%d").map_err(|_| format!("{what} must be a date like 2026-01-31"))
    };
    Ok(match range {
        "today" => (today, today),
        "week" => {
            // Weeks run Saturday → Friday.
            let since_sat = (today.weekday().num_days_from_monday() + 2) % 7;
            let start = today - Duration::days(since_sat as i64);
            (start, start + Duration::days(6))
        }
        "month" => {
            let start = today.with_day(1).unwrap();
            let next = if start.month() == 12 {
                NaiveDate::from_ymd_opt(start.year() + 1, 1, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1).unwrap()
            };
            (start, next - Duration::days(1))
        }
        "custom" => {
            let (a, b) = (parse(from, "from")?, parse(to, "to")?);
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            if (b - a).num_days() > 3660 {
                return Err("the range can be at most 10 years".into());
            }
            (a, b)
        }
        other => return Err(format!("unknown range {other}")),
    })
}

fn local_midnight_utc(tz: chrono_tz::Tz, d: NaiveDate) -> DateTime<Utc> {
    let naive = d.and_hms_opt(0, 0, 0).unwrap();
    tz.from_local_datetime(&naive)
        .earliest()
        // A DST gap at midnight: the day starts an hour later.
        .or_else(|| tz.from_local_datetime(&(naive + Duration::hours(1))).earliest())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
}

fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn user_key(r: &Row) -> String {
    match (r.user_id, &r.author) {
        (Some(id), _) => id.to_string(),
        (None, Some(a)) if !a.is_empty() => format!("ext:{a}"),
        _ => "none".into(),
    }
}

pub async fn report(State(st): State<St>, AuthUser(me): AuthUser, Query(p): Query<Params>) -> ApiResult<Json<Value>> {
    let tz = settings::tz(&st.db);
    let today = Utc::now().with_timezone(&tz).date_naive();
    let range = p.range.as_deref().unwrap_or("month");
    let (first, last) = day_range(range, today, p.from.as_deref(), p.to.as_deref()).map_err(ApiError::bad)?;
    let start = local_midnight_utc(tz, first);
    let end = local_midnight_utc(tz, last + Duration::days(1));

    // Users only ever see their own usage.
    let user_filter = if me.is_admin() { p.user.clone().filter(|u| !u.is_empty()) } else { Some(me.id.to_string()) };

    let rows: Vec<Row> = {
        let c = st.db.conn();
        let mut q = c.prepare(
            "SELECT run_id,chat_id,project_id,user_id,author,schedule_id,source,tool,model,input_tokens,output_tokens,
                    cache_read_tokens,cache_write_tokens,reasoning_tokens,web_search_requests,cost_usd,legacy,at
             FROM run_usage WHERE at >= ?1 AND at < ?2 ORDER BY at",
        )?;
        let it = q.query_map([iso(start), iso(end)], |r| {
            Ok(Row {
                run_id: r.get(0)?,
                chat_id: r.get(1)?,
                project_id: r.get(2)?,
                user_id: r.get(3)?,
                author: r.get(4)?,
                schedule_id: r.get(5)?,
                source: r.get(6)?,
                tool: r.get(7)?,
                model: r.get(8)?,
                input: r.get(9)?,
                output: r.get(10)?,
                cache_read: r.get(11)?,
                cache_write: r.get(12)?,
                reasoning: r.get(13)?,
                web_search: r.get(14)?,
                cost_usd: r.get(15)?,
                legacy: r.get::<_, i64>(16)? != 0,
                at: r.get(17)?,
            })
        })?;
        it.collect::<Result<_, _>>()?
    };

    let names = Names::load(&st)?;
    let price_list = prices(&st);

    let mut total = Agg::default();
    let mut days: BTreeMap<NaiveDate, Agg> = BTreeMap::new();
    let mut d = first;
    while d <= last {
        days.insert(d, Agg::default());
        d += Duration::days(1);
    }
    let mut projects: HashMap<i64, Agg> = HashMap::new();
    let mut users: HashMap<String, Agg> = HashMap::new();
    let mut models: HashMap<(String, String), Agg> = HashMap::new();
    let mut tools: HashMap<String, Agg> = HashMap::new();
    let mut sources: HashMap<String, Agg> = HashMap::new();
    let mut chats: HashMap<i64, (i64, Agg)> = HashMap::new();
    let mut schedules: HashMap<i64, (i64, Agg)> = HashMap::new();
    let mut user_projects: HashMap<(String, i64), Agg> = HashMap::new();
    let mut unpriced: HashSet<String> = HashSet::new();
    let mut legacy_rows = 0usize;

    for r in &rows {
        let ukey = user_key(r);
        if let Some(f) = &user_filter {
            if &ukey != f {
                continue;
            }
        }
        if p.project_id.is_some_and(|id| id != r.project_id) {
            continue;
        }
        if p.tool.as_deref().is_some_and(|t| !t.is_empty() && t != r.tool) {
            continue;
        }
        let (cost, estimated) = match r.cost_usd {
            Some(c) => (c, false),
            None => match price_for(&price_list, &r.model) {
                Some(pr) => (
                    (r.input as f64 * pr.input
                        + r.cache_read as f64 * pr.cache_read
                        + r.cache_write as f64 * pr.cache_write
                        + r.output as f64 * pr.output)
                        / 1_000_000.0,
                    true,
                ),
                None => {
                    unpriced.insert(r.model.clone());
                    (0.0, false)
                }
            },
        };
        if r.legacy {
            legacy_rows += 1;
        }
        total.add(r, cost, estimated);
        if let Some(day) = DateTime::parse_from_rfc3339(&r.at).ok().map(|t| t.with_timezone(&tz).date_naive()) {
            days.entry(day).or_default().add(r, cost, estimated);
        }
        projects.entry(r.project_id).or_default().add(r, cost, estimated);
        users.entry(ukey.clone()).or_default().add(r, cost, estimated);
        user_projects.entry((ukey, r.project_id)).or_default().add(r, cost, estimated);
        models.entry((r.tool.clone(), r.model.clone())).or_default().add(r, cost, estimated);
        tools.entry(r.tool.clone()).or_default().add(r, cost, estimated);
        sources.entry(r.source.clone()).or_default().add(r, cost, estimated);
        chats.entry(r.chat_id).or_insert_with(|| (r.project_id, Agg::default())).1.add(r, cost, estimated);
        if let Some(sid) = r.schedule_id {
            schedules.entry(sid).or_insert_with(|| (r.project_id, Agg::default())).1.add(r, cost, estimated);
        }
    }

    let with = |a: &Agg, extra: Value| {
        let mut v = serde_json::to_value(a).unwrap_or_default();
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        v
    };
    let by_cost = |v: &mut Vec<Value>| {
        v.sort_by(|a, b| {
            let k = |x: &Value| (x["cost"].as_f64().unwrap_or(0.0), x["total_tokens"].as_i64().unwrap_or(0));
            k(b).partial_cmp(&k(a)).unwrap_or(std::cmp::Ordering::Equal)
        })
    };

    let mut project_rows: Vec<Value> = projects.iter().map(|(id, a)| with(a, json!({"id": id, "name": names.project(*id)}))).collect();
    by_cost(&mut project_rows);

    let mut user_rows: Vec<Value> = users
        .iter()
        .map(|(key, a)| {
            let (name, role, kind) = names.user(key);
            let mut per_project: Vec<Value> = user_projects
                .iter()
                .filter(|((k, _), _)| k == key)
                .map(|((_, pid), a)| with(a, json!({"id": pid, "name": names.project(*pid)})))
                .collect();
            by_cost(&mut per_project);
            with(a, json!({"key": key, "name": name, "role": role, "kind": kind, "projects": per_project}))
        })
        .collect();
    by_cost(&mut user_rows);

    let mut model_rows: Vec<Value> = models.iter().map(|((tool, model), a)| with(a, json!({"tool": tool, "model": model}))).collect();
    by_cost(&mut model_rows);
    let mut tool_rows: Vec<Value> = tools.iter().map(|(t, a)| with(a, json!({"tool": t}))).collect();
    by_cost(&mut tool_rows);
    let mut source_rows: Vec<Value> = sources.iter().map(|(s, a)| with(a, json!({"source": s}))).collect();
    by_cost(&mut source_rows);
    let mut chat_rows: Vec<Value> = chats
        .iter()
        .map(|(id, (pid, a))| {
            let (title, exists) = names.chat(*id);
            with(a, json!({"id": id, "title": title, "exists": exists, "project_id": pid, "project": names.project(*pid)}))
        })
        .collect();
    by_cost(&mut chat_rows);
    chat_rows.truncate(200);
    let mut schedule_rows: Vec<Value> = schedules
        .iter()
        .map(|(id, (pid, a))| with(a, json!({"id": id, "name": names.schedule(*id), "project_id": pid, "project": names.project(*pid)})))
        .collect();
    by_cost(&mut schedule_rows);

    // Everyone the viewer may filter by (admins: all accounts, even those with no usage yet).
    let user_options: Vec<Value> = if me.is_admin() {
        let mut o: Vec<Value> = names.users.iter().map(|(id, (n, role))| json!({"key": id.to_string(), "name": n, "role": role})).collect();
        for k in users.keys().filter(|k| k.parse::<i64>().map(|id| !names.users.contains_key(&id)).unwrap_or(true)) {
            let (n, role, _) = names.user(k);
            o.push(json!({"key": k, "name": n, "role": role}));
        }
        o.sort_by_key(|v| v["name"].as_str().unwrap_or("").to_lowercase());
        o
    } else {
        vec![json!({"key": me.id.to_string(), "name": me.username, "role": me.role})]
    };

    let mut unpriced: Vec<String> = unpriced.into_iter().collect();
    unpriced.sort();
    Ok(Json(json!({
        "timezone": tz.name(),
        "range": range,
        "from": first.to_string(),
        "to": last.to_string(),
        "today": today.to_string(),
        "start_utc": iso(start),
        "end_utc": iso(end),
        "scope": if me.is_admin() { "all" } else { "self" },
        "filters": {"user": user_filter, "project_id": p.project_id, "tool": p.tool},
        "totals": total,
        "days": days.iter().map(|(d, a)| with(a, json!({"date": d.to_string()}))).collect::<Vec<_>>(),
        "projects": project_rows,
        "users": user_rows,
        "models": model_rows,
        "tools": tool_rows,
        "sources": source_rows,
        "chats": chat_rows,
        "schedules": schedule_rows,
        "user_options": user_options,
        "unpriced_models": unpriced,
        "legacy_rows": legacy_rows,
    })))
}

/// Display names, with fallbacks for things deleted since.
struct Names {
    projects: HashMap<i64, String>,
    users: BTreeMap<i64, (String, String)>,
    chats: HashMap<i64, String>,
    schedules: HashMap<i64, String>,
}

impl Names {
    fn load(st: &St) -> anyhow::Result<Names> {
        let c = st.db.conn();
        let pairs = |sql: &str| -> anyhow::Result<HashMap<i64, String>> {
            let mut q = c.prepare(sql)?;
            let it = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            Ok(it.collect::<Result<_, _>>()?)
        };
        let projects = pairs("SELECT id,name FROM projects")?;
        let chats = pairs("SELECT id,title FROM chats")?;
        let schedules = pairs("SELECT id,name FROM schedules")?;
        let mut q = c.prepare("SELECT id,username,role FROM users")?;
        let users = q
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, (r.get::<_, String>(1)?, r.get::<_, String>(2)?))))?
            .collect::<Result<_, _>>()?;
        Ok(Names { projects, users, chats, schedules })
    }

    fn project(&self, id: i64) -> String {
        self.projects.get(&id).cloned().unwrap_or_else(|| format!("Deleted project #{id}"))
    }

    fn chat(&self, id: i64) -> (String, bool) {
        match self.chats.get(&id) {
            Some(t) => (t.clone(), true),
            None => (format!("Deleted chat #{id}"), false),
        }
    }

    fn schedule(&self, id: i64) -> String {
        self.schedules.get(&id).cloned().unwrap_or_else(|| format!("Deleted schedule #{id}"))
    }

    /// (name, role, kind) where kind is account | external | system.
    fn user(&self, key: &str) -> (String, String, &'static str) {
        if let Some(name) = key.strip_prefix("ext:") {
            return (name.to_string(), String::new(), "external");
        }
        if key == "none" {
            return ("System".into(), String::new(), "system");
        }
        match key.parse::<i64>().ok().and_then(|id| self.users.get(&id).map(|u| (id, u))) {
            Some((_, (n, r))) => (n.clone(), r.clone(), "account"),
            None => (format!("Deleted user #{key}"), String::new(), "account"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches() {
        assert!(glob("*", "anything"));
        assert!(glob("gpt-5*", "gpt-5.1-codex"));
        assert!(glob("*-mini*", "gpt-5.1-codex-mini"));
        assert!(glob("GPT-5.2*", "gpt-5.2-codex"));
        assert!(!glob("gpt-5.2*", "gpt-5.1-codex"));
        assert!(glob("o3", "o3"));
        assert!(!glob("o3", "o3-pro"));
    }

    #[test]
    fn exact_name_then_first_matching_pattern() {
        let list = parse_prices(
            r#"[
              {"model":"*-mini*","input":0.25,"output":2},
              {"model":"*","input":9,"output":9},
              {"model":"gpt-5.1-codex-mini","input":0.3,"output":3},
              {"model":"gpt-5.1*","input":1.25,"output":10}
            ]"#,
        )
        .unwrap();
        assert_eq!(price_for(&list, "gpt-5.1-codex-mini").unwrap().input, 0.3);
        assert_eq!(price_for(&list, "GPT-5.1-codex-mini").unwrap().input, 0.3);
        assert_eq!(price_for(&list, "gpt-5.4-mini").unwrap().model, "*-mini*");
        assert_eq!(price_for(&list, "gpt-5.1-codex").unwrap().model, "*");
        assert_eq!(price_for(&list, "default").unwrap().model, "*");
        let builtin = parse_prices(settings::DEFAULT_PRICES).unwrap();
        assert_eq!(price_for(&builtin, "claude-opus-5-5[1m]").unwrap().model, "claude-opus-5-5");
        assert_eq!(price_for(&builtin, "claude-opus-9-9").unwrap().model, "claude-opus*");
        assert!(parse_prices(r#"[{"model":"x","input":-1}]"#).is_err());
        assert!(parse_prices(r#"[{"model":"","input":1}]"#).is_err());
    }

    #[test]
    fn weeks_start_on_saturday() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        // 2026-10-27 is a Tuesday.
        assert_eq!(day_range("week", d("2026-10-27"), None, None).unwrap(), (d("2026-10-24"), d("2026-10-30")));
        assert_eq!(day_range("week", d("2026-10-24"), None, None).unwrap(), (d("2026-10-24"), d("2026-10-30")));
        assert_eq!(day_range("week", d("2026-10-30"), None, None).unwrap(), (d("2026-10-24"), d("2026-10-30")));
        assert_eq!(day_range("month", d("2026-12-15"), None, None).unwrap(), (d("2026-12-01"), d("2026-12-31")));
        assert_eq!(day_range("custom", d("2026-10-27"), Some("2026-10-05"), Some("2026-10-01")).unwrap(), (d("2026-10-01"), d("2026-10-05")));
        assert!(day_range("custom", d("2026-10-27"), Some("bad"), Some("2026-10-01")).is_err());
    }

    #[test]
    fn day_starts_at_local_midnight() {
        let t = local_midnight_utc(chrono_tz::Asia::Tehran, NaiveDate::from_ymd_opt(2026, 10, 27).unwrap());
        assert_eq!(iso(t), "2026-10-26T20:30:00Z");
    }

    #[test]
    fn claude_usage_is_split_per_turn() {
        use crate::agents::ModelUsage;
        let m1 = json!({"inputTokens": 2, "outputTokens": 51, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 37562, "costUSD": 0.0075});
        let m2 = json!({"inputTokens": 4, "outputTokens": 69, "cacheReadInputTokens": 28514, "cacheCreationInputTokens": 46690, "costUSD": 0.0096});
        let a = ModelUsage::from_claude("haiku", &m1);
        let b = ModelUsage::from_claude("haiku", &m2).minus(Some(&a));
        assert_eq!((b.input, b.output, b.cache_read, b.cache_write), (2, 18, 28514, 9128));
        assert!((b.cost_usd.unwrap() - 0.0021).abs() < 1e-9);
    }
}
