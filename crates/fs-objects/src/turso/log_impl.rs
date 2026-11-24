//! [`ActivityLog`] for the Turso backend: log-only entries and reading the log.
//!
//! Pattern: Repository (log) — append and filter activities; the hub reads with a cursor.

use async_trait::async_trait;
use turso::Value;

use super::TursoStore;
use super::codec::decode_activity;
use super::guard::refuse_unlogged;
use super::schema::ACTIVITY_COLS;
use super::sql::collect;
use super::tx::in_tx;
use super::write_by::require_owner;
use crate::{Activity, ActivityFilter, ActivityKind, ActivityLog, ChangeFilter, ObjectId, Result};

#[async_trait]
impl ActivityLog for TursoStore {
    async fn log(
        &self,
        kind: ActivityKind,
        object: ObjectId,
        actor: &str,
        payload: serde_json::Value,
    ) -> Result<Activity> {
        let mut inner = self.inner.lock().await;
        let inner = &mut *inner;
        require_owner(inner)?;
        let conn = inner.conn.clone();
        refuse_unlogged(&conn, object).await?;
        let act = in_tx(&conn, self.append_log(inner, actor, object, kind, payload)).await?;
        self.publish(std::slice::from_ref(&act));
        Ok(act)
    }

    async fn activities(&self, f: &ActivityFilter) -> Result<Vec<Activity>> {
        let inner = self.inner.lock().await;
        let mut clauses: Vec<&str> = Vec::new();
        let mut p: Vec<Value> = Vec::new();
        if let Some(a) = f.after {
            clauses.push("id > ?");
            p.push(Value::Text(a.to_string()));
        }
        if let Some(o) = f.object {
            clauses.push("object_id = ?");
            p.push(Value::Text(o.to_string()));
        }
        if let Some(a) = &f.actor {
            clauses.push("actor = ?");
            p.push(Value::Text(a.clone()));
        }
        if f.unsynced_only {
            clauses.push("synced = 0");
        }
        let w = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        p.push(Value::Integer(f.limit.map_or(i64::MAX, |l| l as i64)));
        let rows = collect(
            &inner.conn,
            &format!("SELECT {ACTIVITY_COLS} FROM activities{w} ORDER BY id LIMIT ?"),
            p,
        )
        .await?;
        rows.iter().map(|r| decode_activity(r)).collect()
    }

    async fn changes_by(&self, f: &ChangeFilter) -> Result<Vec<Activity>> {
        if f.objects.as_ref().is_some_and(Vec::is_empty) {
            return Ok(vec![]);
        }
        let inner = self.inner.lock().await;
        let mut clauses: Vec<String> = vec![
            "kind IN ('Create','Update','Delete','Undo')".into(),
            "unit IS NULL".into(),
        ];
        let mut p: Vec<Value> = Vec::new();
        if let Some(a) = f.after {
            clauses.push("id > ?".into());
            p.push(Value::Text(a.to_string()));
        }
        if let Some(x) = &f.program {
            clauses.push("origin_program = ?".into());
            p.push(Value::Text(x.clone()));
        }
        if let Some(x) = &f.run {
            clauses.push("context = ?".into());
            p.push(Value::Text(x.clone()));
        }
        if let Some(objs) = &f.objects {
            // the head itself, or any member of its unit, is about one of the objects
            let marks = vec!["?"; objs.len()].join(",");
            clauses.push(format!(
                "(object_id IN ({marks}) OR id IN (SELECT unit FROM activities \
                 WHERE unit IS NOT NULL AND object_id IN ({marks})))"
            ));
            for _ in 0..2 {
                p.extend(objs.iter().map(|o| Value::Text(o.to_string())));
            }
        }
        p.push(Value::Integer(f.limit.map_or(i64::MAX, |l| l as i64)));
        let rows = collect(
            &inner.conn,
            &format!(
                "SELECT {ACTIVITY_COLS} FROM activities WHERE {} ORDER BY id LIMIT ?",
                clauses.join(" AND ")
            ),
            p,
        )
        .await?;
        rows.iter().map(|r| decode_activity(r)).collect()
    }
}
