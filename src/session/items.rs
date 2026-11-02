//! Bounded session history and opt-in native item notifications.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};
use tokio::sync::Notify;

const MAX_ITEMS: usize = 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_ITEM_BYTES: usize = 4 * 1024 * 1024;
const MAX_PUSH_ITEMS: usize = 256;
const MAX_PUSH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct Journal {
    items: VecDeque<(u64, Value, usize)>,
    bytes: usize,
    cursor: u64,
    dropped: u64,
    last_dropped: u64,
}

pub(crate) struct SessionItems {
    id: String,
    journal: Mutex<Journal>,
    push: Option<Arc<PushQueue>>,
}

impl SessionItems {
    pub fn new(id: String, push: Option<Arc<PushQueue>>) -> Self {
        Self {
            id,
            journal: Mutex::new(Journal::default()),
            push,
        }
    }

    pub fn record(&self, kind: &str, data: Value, turn_id: Option<&str>) {
        let mut journal = self.journal.lock().unwrap();
        journal.cursor += 1;
        let cursor = journal.cursor;
        let mut item =
            json!({"session_id":self.id,"turn_id":turn_id,"cursor":cursor,"kind":kind,"data":data});
        let mut bytes = item.to_string().len();
        if bytes > MAX_ITEM_BYTES {
            item["kind"] = json!("omitted");
            item["data"] =
                json!({"kind":kind,"bytes":bytes,"reason":"item exceeds 4 MiB retention limit"});
            bytes = item.to_string().len();
            journal.dropped += 1;
            journal.last_dropped = cursor;
        }
        while journal.items.len() >= MAX_ITEMS || journal.bytes + bytes > MAX_BYTES {
            if let Some((cursor, _, bytes)) = journal.items.pop_front() {
                journal.bytes -= bytes;
                journal.dropped += 1;
                journal.last_dropped = journal.last_dropped.max(cursor);
            }
        }
        journal.bytes += bytes;
        journal.items.push_back((cursor, item.clone(), bytes));
        // Keep enqueue order identical to journal order across producer tasks.
        if let Some(push) = &self.push {
            push.enqueue(item, bytes);
        }
    }

    pub fn read(&self, after: u64, limit: usize) -> Result<Value, String> {
        if !(1..=256).contains(&limit) {
            return Err("limit must be between 1 and 256".into());
        }
        let journal = self.journal.lock().unwrap();
        if after > journal.cursor {
            return Err("after exceeds this session's item cursor".into());
        }
        let mut items = Vec::new();
        let mut bytes = 0;
        let mut cursor = after;
        for (seq, value, size) in journal.items.iter().filter(|(seq, _, _)| *seq > after) {
            if items.len() >= limit || (!items.is_empty() && bytes + size > MAX_ITEM_BYTES) {
                break;
            }
            items.push(value.clone());
            bytes += size;
            cursor = *seq;
        }
        Ok(json!({
            "session_id":self.id,"items":items,"cursor":cursor,
            "latest_cursor":journal.cursor,"has_more":cursor < journal.cursor,
            "items_lost":after < journal.last_dropped,"dropped_items":journal.dropped,
        }))
    }
}

#[derive(Default)]
struct Pending {
    items: VecDeque<(Value, usize)>,
    bytes: usize,
    dropped: u64,
    closed: bool,
}

/// Producers never wait on stdout. Slow consumers lose oldest queued items;
/// the independent session journal remains available for cursor recovery.
#[derive(Default)]
pub(crate) struct PushQueue {
    pending: Mutex<Pending>,
    changed: Notify,
}

impl PushQueue {
    fn enqueue(&self, item: Value, bytes: usize) {
        let mut pending = self.pending.lock().unwrap();
        if pending.closed {
            return;
        }
        while pending.items.len() >= MAX_PUSH_ITEMS || pending.bytes + bytes > MAX_PUSH_BYTES {
            if let Some((_, bytes)) = pending.items.pop_front() {
                pending.bytes -= bytes;
                pending.dropped += 1;
            }
        }
        pending.bytes += bytes;
        pending.items.push_back((item, bytes));
        drop(pending);
        self.changed.notify_one();
    }

    pub async fn next(&self) -> Option<Value> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut pending = self.pending.lock().unwrap();
                if let Some((mut item, bytes)) = pending.items.pop_front() {
                    pending.bytes -= bytes;
                    item["push_dropped"] = json!(pending.dropped);
                    return Some(item);
                }
                if pending.closed {
                    return None;
                }
            }
            changed.await;
        }
    }

    pub fn close(&self) {
        self.pending.lock().unwrap().closed = true;
        self.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_spans_turns_and_paginates_without_skipping_items() {
        let items = SessionItems::new("s".into(), None);
        items.record(
            "native",
            json!({"nested":{"unknown":[1,{"key":true}]}}),
            Some("a"),
        );
        items.record("turn_finished", json!({"text":"answer"}), Some("a"));
        items.record("turn_started", json!({"prompt":"next"}), Some("b"));
        let first = items.read(0, 2).unwrap();
        assert_eq!(first["cursor"], 2);
        assert_eq!(first["has_more"], true);
        assert_eq!(
            first["items"][0]["data"]["nested"]["unknown"][1]["key"],
            true
        );
        let second = items.read(2, 2).unwrap();
        assert_eq!(second["items"][0]["turn_id"], "b");
        assert_eq!(second["cursor"], 3);
        assert_eq!(second["has_more"], false);
        assert!(items.read(4, 2).is_err());
        assert!(items.read(0, 0).is_err());
    }

    #[tokio::test]
    async fn slow_push_consumer_loses_oldest_but_can_recover_from_history() {
        let push = Arc::new(PushQueue::default());
        let items = SessionItems::new("s".into(), Some(push.clone()));
        for n in 0..300 {
            items.record("native", json!({"n":n}), Some("a"));
        }
        let notification = push.next().await.unwrap();
        assert_eq!(notification["cursor"], 45);
        assert_eq!(notification["push_dropped"], 44);
        let history = items.read(0, 256).unwrap();
        assert_eq!(history["items"][0]["cursor"], 1);
        assert_eq!(history["items_lost"], false);
        push.close();
        let mut last = notification;
        while let Some(item) = push.next().await {
            last = item;
        }
        assert_eq!(last["cursor"], 300);
    }

    #[test]
    fn retention_reports_gaps_and_oversized_items_explicitly() {
        let items = SessionItems::new("s".into(), None);
        for n in 0..MAX_ITEMS + 3 {
            items.record("native", json!(n), None);
        }
        let first = items.read(0, 256).unwrap();
        assert_eq!(first["items"][0]["cursor"], 4);
        assert_eq!(first["items_lost"], true);
        assert_eq!(first["dropped_items"], 3);
        assert_eq!(items.read(3, 1).unwrap()["items_lost"], false);
        items.record("native", json!("x".repeat(MAX_ITEM_BYTES)), None);
        let last = items.read((MAX_ITEMS + 3) as u64, 1).unwrap();
        assert_eq!(last["items"][0]["kind"], "omitted");
        assert_eq!(last["items"][0]["data"]["kind"], "native");
        assert_eq!(last["items_lost"], true);
        assert_eq!(last["cursor"], MAX_ITEMS + 4);
    }

    #[tokio::test]
    async fn byte_limits_bound_large_payloads_and_keep_terminal_item() {
        let push = Arc::new(PushQueue::default());
        let items = SessionItems::new("s".into(), Some(push.clone()));
        for _ in 0..10 {
            items.record("native", json!("x".repeat(1024 * 1024)), None);
        }
        items.record("turn_finished", json!({"status":"succeeded"}), Some("a"));
        assert!(items.journal.lock().unwrap().bytes <= MAX_BYTES);
        assert!(push.pending.lock().unwrap().bytes <= MAX_PUSH_BYTES);
        assert!(push.pending.lock().unwrap().dropped > 0);
        push.close();
        let mut last = None;
        while let Some(item) = push.next().await {
            last = Some(item);
        }
        assert_eq!(last.unwrap()["kind"], "turn_finished");
    }
}
