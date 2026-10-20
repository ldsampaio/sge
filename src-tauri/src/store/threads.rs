//! Per-folder JWZ thread-id epoch via `mail-threading` 0.1.x
//! (Phase 42, Plan 42-04 — THR-02/D-03/D-04).
//!
//! The wrapper owns everything the crate intentionally does not:
//! folder scoping (`WHERE mailbox_id = ?` — THR-04 says `thread_key =
//! (mailbox_id, thread_id)`; the same root id may legitimately head
//! distinct threads in two folders), the deterministic `(date_utc, uid)`
//! member re-sort (the crate documents no member order), and idempotent
//! batched UPDATEs — one rusqlite transaction, never per-insert. Hook
//! sites (sync-end / backfill-complete / send-file) are wired by Plan 42-05.
//!
//! D-03 gate protocol: if a gate fixture below FAILS behaviorally (not a
//! compile error), the documented fallback is an in-house ~250-LOC JWZ
//! core validated against the same RFC 5256 corpus — engage it through a
//! recorded decision, never by silently swapping implementations.

use std::collections::HashMap;

use chrono::DateTime;
use chrono::Utc;
use mail_threading::{thread_messages_with, Message, ThreadingOptions};
use rusqlite::{params, Connection};

use super::{StoreError, StoreResult};

/// Group identity (D-04): `thread_key = (mailbox_id, thread_id)`.
///
/// Bare `thread_id` is a within-folder detail — it must never be consumed
/// as a grouping key outside the folder scope. Build one through
/// [`thread_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThreadKey {
    pub mailbox_id: u64,
    pub thread_id: u64,
}

/// Pair a folder with its local thread id (the D-04 grouping contract).
pub fn thread_key(mailbox_id: u64, thread_id: u64) -> ThreadKey {
    ThreadKey {
        mailbox_id,
        thread_id,
    }
}

/// What one epoch pass did (consumers log it; hooks in Plan 42-05 sum it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecomputeStats {
    /// Rows scanned in the folder scope (`pending_delete = 0`).
    pub messages_seen: usize,
    /// Threads returned by the JWZ compute.
    pub threads_seen: usize,
    /// Rows whose `thread_id` changed and were written this pass.
    pub rows_written: usize,
}

/// One folder-scoped DB row projected to the epoch's needs.
struct FolderRow {
    id: u64,
    uid: u32,
    date: DateTime<Utc>,
    /// `in_reply_to` present or `references_json` decoded non-empty.
    has_chain: bool,
    /// Current persisted `thread_id` (diff base — idempotency).
    current: Option<u64>,
}

/// Proposed UPDATEs for one thread, plus its persisted member order.
struct ThreadPlan {
    thread_id: u64,
    /// All thread members, sorted `(date_utc, uid)` DESC — the order we
    /// persist by (never the crate's member order, which is unspecified).
    members: Vec<u64>,
    /// Members whose stored `thread_id` differs from `thread_id`.
    updates: Vec<u64>,
}

/// Deterministic fallback when `date_utc` fails RFC3339 parse — Parser rows
/// are formatter-written, so this path is an invariant-violation softener,
/// not a correctness branch.
fn fallback_date() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default()
}

/// Decode a stored normalized `references_json` array; unparseable rows
/// fall back to the declared shallow chain (References = [In-Reply-To]).
fn decode_references(raw: Option<&str>, in_reply_to: &Option<String>) -> Vec<String> {
    let mut refs: Vec<String> = raw
        .and_then(|r| serde_json::from_str::<Vec<String>>(r).ok())
        .unwrap_or_default();
    if refs.is_empty() {
        if let Some(irt) = in_reply_to {
            refs.push(irt.clone());
        }
    }
    refs
}

/// Parse the internal RFC3339 `date_utc`; failures fall back to epoch-zero
/// (deterministic softener — rows are formatter-written, never hand-made).
fn parse_date_utc(date_utc: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(date_utc)
        .map(|d| d.to_utc())
        .unwrap_or_else(|_| fallback_date())
}

/// Load the folder scope and project each row two ways: the diff/order
/// view (`by_id`) plus the crate's compute view (`msgs`). The scoped SELECT
/// reuses the exact `list_messages` predicate fragment (WHERE mailbox_id = ?
/// AND pending_delete = 0) — `u64 messages.id` ↔ String caller-id is
/// `id.to_string()`, reverse-mapped from caller ids on output.
fn load_folder_rows(
    conn: &Connection,
    mailbox_id: u64,
) -> StoreResult<(HashMap<u64, FolderRow>, Vec<Message>)> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.id, m.uid, m.message_id, m.in_reply_to, m.references_json, \
         m.date_utc, m.subject, m.thread_id \
         FROM messages m WHERE m.mailbox_id = ?1 AND m.pending_delete = 0 \
         ORDER BY m.id",
    )?;
    let rows = stmt.query_map(params![mailbox_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<i64>>(7)?,
        ))
    })?;

    let mut by_id: HashMap<u64, FolderRow> = HashMap::new();
    let mut msgs = Vec::new();
    for row in rows {
        let (id, uid, message_id, in_reply_to, references_json, date_utc, subject, current) = row?;
        let id = id.max(0) as u64;
        let refs = decode_references(references_json.as_deref(), &in_reply_to);
        let date = parse_date_utc(&date_utc);
        by_id.insert(
            id,
            FolderRow {
                id,
                uid: uid.max(0) as u32,
                date,
                has_chain: in_reply_to.is_some() || !refs.is_empty(),
                current: current.map(|v| v.max(0) as u64),
            },
        );
        msgs.push(Message {
            id: id.to_string(),
            message_id,
            in_reply_to,
            references: refs,
            subject: subject.unwrap_or_default(),
            date,
        });
    }
    Ok((by_id, msgs))
}

/// Recompute thread ids for one folder — the D-04 epoch.
///
/// `newly_seen_uids` is the caller's dirty set (hook sites in Plan 42-05):
/// a non-empty set runs the full per-folder epoch (a fresh reply needs its
/// root's rows), an empty set exits early touching zero rows. Never
/// per-insert. Writes, when any, are batched in ONE transaction.
pub fn recompute_threads(
    conn: &mut Connection,
    mailbox_id: u64,
    newly_seen_uids: Option<&[u32]>,
) -> StoreResult<RecomputeStats> {
    if newly_seen_uids.is_some_and(|set| set.is_empty()) {
        return Ok(RecomputeStats {
            messages_seen: 0,
            threads_seen: 0,
            rows_written: 0,
        });
    }
    let (by_id, msgs) = load_folder_rows(conn, mailbox_id)?;
    let plans = compute_epoch_plans(msgs, &by_id);
    let updates = planned_updates(&plans);
    if updates.is_empty() {
        // Changed-rows count 0 → exit before any transaction (zero WAL churn).
        return Ok(RecomputeStats {
            messages_seen: by_id.len(),
            threads_seen: plans.len(),
            rows_written: 0,
        });
    }
    let rows_written = apply_epoch_updates(conn, &updates)?;
    Ok(RecomputeStats {
        messages_seen: by_id.len(),
        threads_seen: plans.len(),
        rows_written,
    })
}

/// The JWZ compute pass (D-03): run the crate once per folder, re-sort each
/// thread's members by the project's `(date_utc, uid)` DESC pair (the same
/// tuple as `queries.rs list_messages` — never the crate's unspecified
/// member order), and reject subject-only grouping with `subject_merge:
/// false` (the D-04 persist order is ours).
fn compute_epoch_plans(msgs: Vec<Message>, by_id: &HashMap<u64, FolderRow>) -> Vec<ThreadPlan> {
    let options = ThreadingOptions {
        subject_merge: false,
        ..ThreadingOptions::default()
    };
    let mut plans = Vec::new();
    if !msgs.is_empty() {
        for thread in thread_messages_with(&msgs, &options) {
            if let Some(plan) = build_thread_plan(&thread, by_id) {
                plans.push(plan);
            }
        }
    }
    plans
}

/// Build the persisted plan for one returned thread: parse caller ids back
/// to `messages.id` (u64), re-sort members `(date_utc, uid)` DESC, and diff
/// against the stored `thread_id`. NULL thread_id rows with no chain and no
/// linked members stay own-thread sentinels (D-04) and get no UPDATE.
fn build_thread_plan(
    thread: &mail_threading::Thread,
    by_id: &HashMap<u64, FolderRow>,
) -> Option<ThreadPlan> {
    let thread_id = thread.root_message_id.parse::<u64>().ok()?;
    let mut members = thread
        .messages
        .iter()
        .filter_map(|cid| cid.parse::<u64>().ok())
        .collect::<Vec<u64>>();
    sort_members_desc(&mut members, by_id);
    let grouped = members.len() > 1;
    let updates = members
        .iter()
        .filter(|id| match by_id.get(*id) {
            Some(row) => (grouped || row.has_chain) && row.current != Some(thread_id),
            None => false,
        })
        .copied()
        .collect();
    Some(ThreadPlan {
        thread_id,
        members,
        updates,
    })
}

/// Re-sort thread members by the deterministic `(date_utc, uid)` DESC pair
/// (queries.rs:886 analog). The crate documents no member order — persist
/// only after this pass.
fn sort_members_desc(members: &mut [u64], by_id: &HashMap<u64, FolderRow>) {
    members.sort_by(|a, b| {
        let ka = by_id.get(a).map(|r| (r.date, r.uid));
        let kb = by_id.get(b).map(|r| (r.date, r.uid));
        // Lookup-missing rows are impossible from the scoped SELECT; park
        // them after known rows so the sort stays total and deterministic.
        match (kb, ka) {
            (Some(kb), Some(ka)) => kb.cmp(&ka),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.cmp(a),
        }
    });
}

/// Flatten every plan into the single batch list of `id → thread_id` writes.
fn planned_updates(plans: &[ThreadPlan]) -> Vec<(u64, u64)> {
    let mut flat = Vec::new();
    for plan in plans {
        for id in &plan.updates {
            flat.push((*id, plan.thread_id));
        }
    }
    flat
}

/// Apply the batch in ONE rusqlite transaction (rules_io.rs:438 tx shape).
/// WAL churn under the single-writer store mutex is bounded to one commit
/// regardless of folder size; a T-42-04 dirty-set guard backstops this.
fn apply_epoch_updates(conn: &mut Connection, updates: &[(u64, u64)]) -> StoreResult<usize> {
    let tx = conn.transaction().map_err(StoreError::Sql)?;
    let mut written = 0usize;
    {
        let mut stmt = tx
            .prepare_cached("UPDATE messages SET thread_id = ?1 WHERE id = ?2")
            .map_err(StoreError::Sql)?;
        for (id, root) in updates {
            written += stmt
                .execute(params![*root as i64, *id as i64])
                .map_err(StoreError::Sql)?;
        }
    }
    tx.commit().map_err(StoreError::Sql)?;
    Ok(written)
}

#[cfg(test)]
mod recompute_epoch_tests {
    use super::super::Store;
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().expect("in-memory store")
    }

    fn add_mailbox(s: &mut Store, name: &str) -> u64 {
        let conn = s.conn_mut();
        conn.execute("INSERT INTO mailboxes (name) VALUES (?1)", params![name])
            .expect("mailbox insert");
        conn.last_insert_rowid() as u64
    }

    fn add_msg(
        s: &mut Store,
        mb: u64,
        uid: u32,
        message_id: Option<&str>,
        in_reply_to: Option<&str>,
        references: Option<&str>,
        date_utc: &str,
        subject: &str,
    ) -> u64 {
        let conn = s.conn_mut();
        conn.execute(
            "INSERT INTO messages (mailbox_id, uid, message_id, in_reply_to, \
             references_json, date_utc, subject) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                mb,
                uid,
                message_id,
                in_reply_to,
                references,
                date_utc,
                subject
            ],
        )
        .expect("message insert");
        conn.last_insert_rowid() as u64
    }

    fn thread_id_of(s: &mut Store, id: u64) -> Option<u64> {
        s.conn_mut()
            .query_row(
                "SELECT thread_id FROM messages WHERE id = ?1",
                params![id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .expect("row lookup")
            .map(|v| v as u64)
    }

    /// Re-running the epoch on unchanged inputs duplicates nothing and
    /// reshuffles nothing — thread_id assignment is byte-stable.
    #[test]
    fn recompute_epoch_idempotent() {
        let mut s = store();
        let mb = add_mailbox(&mut s, "INBOX");
        let root = add_msg(
            &mut s,
            mb,
            10,
            Some("<r@x>"),
            None,
            None,
            "2026-10-01T10:00:00Z",
            "Hello",
        );
        let reply = add_msg(
            &mut s,
            mb,
            11,
            Some("<p@x>"),
            Some("<r@x>"),
            Some("[\"<r@x>\"]"),
            "2026-10-01T10:05:00Z",
            "Re: Hello",
        );

        let first = recompute_threads(s.conn_mut(), mb, None).expect("epoch");
        assert_eq!(first.messages_seen, 2);
        assert_eq!(first.threads_seen, 1);
        assert_eq!(first.rows_written, 2, "root self-tags and reply joins");
        let map_before = (thread_id_of(&mut s, root), thread_id_of(&mut s, reply));
        assert_eq!(map_before, (Some(root), Some(root)));

        let second = recompute_threads(s.conn_mut(), mb, None).expect("epoch rerun");
        assert_eq!(second.rows_written, 0, "unchanged inputs write zero rows");
        let map_after = (thread_id_of(&mut s, root), thread_id_of(&mut s, reply));
        assert_eq!(map_after, map_before, "thread_id map is byte-identical");
    }

    /// Zero dirty rows → early exit touching zero rows; a no-chain message
    /// never joins a synthetic group (NULL stays the own-thread sentinel).
    #[test]
    fn recompute_early_exit_zero_rows() {
        let mut s = store();
        let mb = add_mailbox(&mut s, "INBOX");
        let lone = add_msg(
            &mut s,
            mb,
            10,
            Some("<l@x>"),
            None,
            None,
            "2026-10-01T10:00:00Z",
            "Solo",
        );

        let dirty = recompute_threads(s.conn_mut(), mb, Some(&[])).expect("early exit");
        assert_eq!(
            dirty,
            RecomputeStats {
                messages_seen: 0,
                threads_seen: 0,
                rows_written: 0
            },
            "empty dirty set exits before the SELECT"
        );
        assert_eq!(thread_id_of(&mut s, lone), None);

        let clean = recompute_threads(s.conn_mut(), mb, None).expect("epoch");
        assert_eq!(clean.rows_written, 0, "unchanged folder writes zero rows");
        assert_eq!(
            thread_id_of(&mut s, lone),
            None,
            "own-thread sentinel holds"
        );
    }

    /// Equal `date_utc` → uid tiebreak decides member order; and with equal
    /// uids the date wins. Order key is exactly `(date_utc DESC, uid DESC)`.
    #[test]
    fn tiebreak_date_uid() {
        let mut s = store();
        let mb = add_mailbox(&mut s, "INBOX");
        let root = add_msg(
            &mut s,
            mb,
            30,
            Some("<r@x>"),
            None,
            None,
            "2026-10-01T09:00:00Z",
            "Hello",
        );
        let r2 = add_msg(
            &mut s,
            mb,
            20,
            Some("<a@x>"),
            Some("<r@x>"),
            Some("[\"<r@x>\"]"),
            "2026-10-01T09:00:00Z",
            "Re: Hello",
        );
        let r3 = add_msg(
            &mut s,
            mb,
            10,
            Some("<b@x>"),
            Some("<a@x>"),
            Some("[\"<r@x>\",\"<a@x>\"]"),
            "2026-10-01T09:00:00Z",
            "Re: Re",
        );

        let st = recompute_threads(s.conn_mut(), mb, None).expect("epoch");
        assert_eq!(st.threads_seen, 1);
        assert_eq!(st.rows_written, 3);
        assert_eq!(
            (
                thread_id_of(&mut s, root),
                thread_id_of(&mut s, r2),
                thread_id_of(&mut s, r3)
            ),
            (Some(root), Some(root), Some(root))
        );

        // Direct member-order proof on the same (date_utc, uid) tuple.
        let date = parse_date_utc("2026-10-01T09:00:00Z");
        let row = |uid| FolderRow {
            id: 0,
            uid,
            date,
            has_chain: true,
            current: None,
        };
        let mut rows = HashMap::new();
        for (id, uid) in [(1u64, 30u32), (2, 20), (3, 10)] {
            rows.insert(id, row(uid));
        }
        let mut members = vec![3u64, 2, 1];
        sort_members_desc(&mut members, &rows);
        assert_eq!(members, vec![1, 2, 3], "uid DESC at equal date_utc");

        // Equal uids, differing dates: the later date sorts first.
        let mut rows2 = HashMap::new();
        rows2.insert(
            7u64,
            FolderRow {
                id: 0,
                uid: 5,
                date: parse_date_utc("2026-11-01T09:00:00Z"),
                has_chain: true,
                current: None,
            },
        );
        rows2.insert(
            8u64,
            FolderRow {
                id: 0,
                uid: 5,
                date: parse_date_utc("2026-10-01T09:00:00Z"),
                has_chain: true,
                current: None,
            },
        );
        let mut members2 = vec![8u64, 7];
        sort_members_desc(&mut members2, &rows2);
        assert_eq!(members2, vec![7, 8], "date DESC at equal uid");
    }

    /// `thread_key = (mailbox_id, thread_id)` never collides across folders:
    /// the same Message-ID chain — including the duplicate-root RESEARCH
    /// gap — seeded into two folders stays two groups, never merged.
    #[test]
    fn thread_key_folder_scoping() {
        let mut s = store();
        let ina = add_mailbox(&mut s, "INBOX");
        let sent = add_mailbox(&mut s, "Sent");
        let root_a = add_msg(
            &mut s,
            ina,
            10,
            Some("<r@x>"),
            None,
            None,
            "2026-10-01T10:00:00Z",
            "Hello",
        );
        let reply_a = add_msg(
            &mut s,
            ina,
            11,
            Some("<p@x>"),
            Some("<r@x>"),
            Some("[\"<r@x>\"]"),
            "2026-10-01T10:05:00Z",
            "Re: Hello",
        );
        let root_b = add_msg(
            &mut s,
            sent,
            10,
            Some("<r@x>"),
            None,
            None,
            "2026-10-02T10:00:00Z",
            "Hello",
        );
        let reply_b = add_msg(
            &mut s,
            sent,
            11,
            Some("<p@x>"),
            Some("<r@x>"),
            Some("[\"<r@x>\"]"),
            "2026-10-02T10:05:00Z",
            "Re: Hello",
        );

        let ina_stats = recompute_threads(s.conn_mut(), ina, None).expect("epoch INBOX");
        let sent_stats = recompute_threads(s.conn_mut(), sent, None).expect("epoch Sent");
        assert_eq!(ina_stats.threads_seen, 1);
        assert_eq!(sent_stats.threads_seen, 1);

        assert_eq!(
            thread_id_of(&mut s, root_a),
            Some(root_a),
            "root scoped to its folder"
        );
        assert_eq!(thread_id_of(&mut s, reply_a), Some(root_a));
        assert_eq!(
            thread_id_of(&mut s, reply_b),
            Some(root_b),
            "reply scoped to its folder"
        );
        assert_ne!(thread_id_of(&mut s, root_a), thread_id_of(&mut s, root_b));

        let key_a = thread_key(ina, root_a);
        let key_b = thread_key(sent, root_b);
        assert_ne!(
            key_a, key_b,
            "duplicate Message-ID roots stay distinct groups"
        );
    }
}
