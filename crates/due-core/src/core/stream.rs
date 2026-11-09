use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use super::{InteractionRef, RunRef, SessionRef};
use crate::session::{ItemReadError, SessionItems};

/// One full retained item, preserving nested native payloads and unknown fields.
#[derive(Debug, Clone, Serialize)]
pub struct SessionItem {
    /// Core logical session, not a native session/thread ID.
    pub session: SessionRef,
    /// Core run, when an item belongs to a turn; None for session-wide items.
    pub run: Option<RunRef>,
    /// Monotonic cursor across the whole session.
    pub cursor: u64,
    /// Native/lifecycle/interaction kind, or explicit `omitted`/closure marker.
    pub kind: String,
    /// Full payload within the documented retention limit; no summary projection.
    pub data: Value,
    /// Exact reply token when this item announced an interaction.
    pub interaction: Option<InteractionRef>,
}

/// An independent cursor over one bounded journal; dropping it never cancels work.
pub struct SessionStream {
    pub(super) session: SessionRef,
    pub(super) items: Arc<SessionItems>,
    pub(super) after: u64,
}

impl SessionStream {
    /// Last delivered item. Save this to resume without consuming another reader.
    pub fn cursor(&self) -> u64 {
        self.after
    }

    /// Await the next item. Cancellation does not advance the cursor.
    /// Returns None after closure and retained items are drained. Lag never skips
    /// silently: create a new subscription at oldest_available - 1 to acknowledge it.
    pub async fn next(&mut self) -> Result<Option<SessionItem>, StreamError> {
        let Some(raw) = self
            .items
            .next_after(self.after)
            .await
            .map_err(StreamError::from)?
        else {
            return Ok(None);
        };
        let cursor = raw["cursor"].as_u64().expect("journal cursor");
        let run = raw["turn_id"].as_str().map(|id| RunRef(id.into()));
        let kind = raw["kind"].as_str().expect("journal kind").to_owned();
        let data = raw["data"].clone();
        let interaction = if kind == "interaction" {
            run.clone()
                .zip(data["request_id"].as_str())
                .map(|(run, id)| InteractionRef {
                    run,
                    request_id: id.into(),
                    cursor,
                })
        } else {
            None
        };
        self.after = cursor;
        Ok(Some(SessionItem {
            session: self.session.clone(),
            run,
            cursor,
            kind,
            data,
            interaction,
        }))
    }
}

/// Retention/invalid cursor errors. Oversized individual items use omission markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    /// The requested cursor predates the oldest retained item.
    Lagged {
        /// Last delivered cursor.
        after: u64,
        /// First retained cursor; resubscribe from one less to acknowledge the gap.
        oldest_available: u64,
        /// Latest produced cursor at observation time.
        latest: u64,
    },
    /// Cursor was ahead of the producer.
    Ahead {
        /// Requested cursor.
        after: u64,
        /// Latest produced cursor.
        latest: u64,
    },
}
impl From<ItemReadError> for StreamError {
    fn from(error: ItemReadError) -> Self {
        match error {
            ItemReadError::Ahead { after, latest } => Self::Ahead { after, latest },
            ItemReadError::Lagged {
                after,
                oldest,
                latest,
            } => Self::Lagged {
                after,
                oldest_available: oldest,
                latest,
            },
        }
    }
}
impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "session stream: {self:?}")
    }
}
impl std::error::Error for StreamError {}
