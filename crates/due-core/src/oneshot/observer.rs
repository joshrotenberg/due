use serde::Serialize;

/// Categories describe observed native activity, not equal units of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Command,
    FileChange,
    McpCall,
    WebSearch,
    PlanUpdate,
    StatusUpdate,
    ToolCall,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

/// Receives native execution progress on the caller's behalf.
pub trait RunObserver: Send + Sync + 'static {
    /// Select streaming execution when true.
    fn enabled(&self) -> bool {
        true
    }
    /// Opt into provider-derived activity and message labels.
    fn detail_enabled(&self) -> bool {
        false
    }
    fn spawned(&self, _pid: u32, _pgid: Option<u32>) {}
    fn session(&self, _id: &str) {}
    fn phase(&self, _event: &'static str) {}
    fn assistant(&self, _bytes: usize, _detail: Option<&str>) {}
    fn activity_started(
        &self,
        _id: Option<&str>,
        _kind: ActivityKind,
        _tool: Option<&str>,
        _detail: Option<&str>,
    ) {
    }
    fn activity_completed(
        &self,
        _id: Option<&str>,
        _kind: ActivityKind,
        _status: ActivityStatus,
        _tool: Option<&str>,
        _detail: Option<&str>,
    ) {
    }
    fn activity_detail(
        &self,
        _id: Option<&str>,
        _kind: ActivityKind,
        _tool: Option<&str>,
        _detail: &str,
    ) {
    }
    /// Reports a native Claude Bash denial's input, which can reflect a hook rewrite.
    /// Delivered only when `detail_enabled` is true.
    fn permission_check_input(&self, _tool_use_id: &str, _command: &str) {}
}

/// Use the buffered path without callbacks.
impl RunObserver for () {
    fn enabled(&self) -> bool {
        false
    }
}
