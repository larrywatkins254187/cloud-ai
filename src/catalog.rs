//! Model catalog read from the installed CLIs, so the UI always lists the
//! models the current Claude Code / Codex versions offer.
use crate::state::St;
use crate::util::now;
use anyhow::Result;
use rusqlite::params;
use serde_json::{json, Value};
use std::time::Duration;

pub async fn refresh(st: &St) -> Value {
    let mut report = json!({});
    match crate::agents::claude::fetch_models(st).await {
        Ok(models) => {
            let rows: Vec<(String, String, String, Value, Option<String>, bool)> = models
                .iter()
                .map(|m| {
                    let id = m["value"].as_str().unwrap_or("").to_string();
                    let mut name = m["displayName"].as_str().unwrap_or(&id).to_string();
                    if let Some(r) = m["resolvedModel"].as_str() {
                        if !name.contains(r) {
                            name = format!("{name} ({r})");
                        }
                    }
                    (
                        id.clone(),
                        name,
                        m["description"].as_str().unwrap_or("").to_string(),
                        m.get("supportedEffortLevels").cloned().unwrap_or(json!([])),
                        None,
                        id == "default",
                    )
                })
                .collect();
            let _ = save(st, "claude", &rows);
            report["claude"] = json!(rows.len());
        }
        Err(e) => {
            report["claude_error"] = json!(format!("{e:#}"));
        }
    }
    match crate::agents::codex::fetch_models(st).await {
        Ok(models) => {
            let rows: Vec<(String, String, String, Value, Option<String>, bool)> = models
                .iter()
                .filter(|m| m["hidden"] != true)
                .map(|m| {
                    let efforts: Vec<Value> = m["supportedReasoningEfforts"]
                        .as_array()
                        .map(|a| a.iter().map(|e| e["reasoningEffort"].clone()).collect())
                        .unwrap_or_default();
                    (
                        m["model"].as_str().or(m["id"].as_str()).unwrap_or("").to_string(),
                        m["displayName"].as_str().unwrap_or("").to_string(),
                        m["description"].as_str().unwrap_or("").to_string(),
                        Value::Array(efforts),
                        m["defaultReasoningEffort"].as_str().map(String::from),
                        m["isDefault"] == true,
                    )
                })
                .collect();
            let _ = save(st, "codex", &rows);
            report["codex"] = json!(rows.len());
        }
        Err(e) => {
            report["codex_error"] = json!(format!("{e:#}"));
        }
    }
    let _ = st.db.set_setting("models_refreshed_at", &now());
    report
}

fn save(st: &St, tool: &str, rows: &[(String, String, String, Value, Option<String>, bool)]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut c = st.db.conn();
    let tx = c.transaction()?;
    tx.execute("DELETE FROM models WHERE tool=?1", [tool])?;
    for (i, (id, name, desc, efforts, def_eff, is_default)) in rows.iter().enumerate() {
        tx.execute(
            "INSERT OR REPLACE INTO models(tool,id,display_name,description,efforts,default_effort,is_default,sort,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![tool, id, name, desc, efforts.to_string(), def_eff, *is_default as i64, i as i64, now()],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn list(st: &St) -> Value {
    let c = st.db.conn();
    let mut q = match c.prepare(
        "SELECT tool,id,display_name,description,efforts,default_effort,is_default FROM models ORDER BY tool, sort",
    ) {
        Ok(q) => q,
        Err(_) => return json!({}),
    };
    let mut out = json!({"claude": [], "codex": []});
    let rows = q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            json!({
                "id": r.get::<_, String>(1)?,
                "name": r.get::<_, String>(2)?,
                "description": r.get::<_, String>(3)?,
                "efforts": serde_json::from_str::<Value>(&r.get::<_, String>(4)?).unwrap_or(json!([])),
                "default_effort": r.get::<_, Option<String>>(5)?,
                "is_default": r.get::<_, i64>(6)? != 0,
            }),
        ))
    });
    if let Ok(rows) = rows {
        for (tool, m) in rows.flatten() {
            if let Some(a) = out[&tool].as_array_mut() {
                a.push(m);
            }
        }
    }
    out["refreshed_at"] = json!(st.db.get_setting("models_refreshed_at"));
    out
}

pub fn spawn(st: St) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            let r = refresh(&st).await;
            tracing::info!("model catalog refreshed: {r}");
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}
