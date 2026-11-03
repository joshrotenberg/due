//! Only validated native message metadata is evidence of a model used this turn.

use serde_json::Value;

use crate::result::ReportedSettings;

pub(crate) fn claude_message(reported: &mut ReportedSettings, event: &Value) {
    // Include native child messages observed during this turn, without claiming
    // that any one ID is the selected root model. Reject malformed envelopes.
    // Init describes configuration; modelUsage can describe session accounting.
    if event
        .get("parent_tool_use_id")
        .is_some_and(|id| !id.is_null() && id.as_str().is_none_or(str::is_empty))
    {
        return;
    }
    let message = match event.get("type").and_then(Value::as_str) {
        Some("assistant") => event.get("message"),
        Some("stream_event")
            if event.pointer("/event/type").and_then(Value::as_str) == Some("message_start") =>
        {
            event.pointer("/event/message")
        }
        _ => None,
    };
    if let Some(message) = message
        && message.get("role").and_then(Value::as_str) == Some("assistant")
        && let Some(model) = message.get("model").and_then(Value::as_str)
        // Reserved placeholder observed in saved native assistant envelopes,
        // not evidence that a model served the message.
        && model != "<synthetic>"
    {
        reported.model(model);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provenance_accepts_only_native_message_models() {
        let mut reported = ReportedSettings::default();
        for event in [
            json!({"type":"system","subtype":"init","model":"configured"}),
            json!({"type":"result","is_error":false,"modelUsage":{"previous":{}}}),
            json!({"type":"future","model":"unknown"}),
            json!({"type":"assistant","message":{"role":"user","model":"wrong-role"}}),
            json!({"type":"assistant","message":{"role":"assistant","model":42}}),
            json!({"type":"assistant","message":{"role":"assistant","model":""}}),
            json!({"type":"assistant","message":{"role":"assistant","model":"\n"}}),
            json!({"type":"assistant","message":{"role":"assistant","model":"<synthetic>"}}),
            json!({"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant","model":"<synthetic>"}}}),
            json!({"type":"assistant","parent_tool_use_id":false,"message":{"role":"assistant","model":"malformed"}}),
            json!({"type":"assistant","message":{"role":"assistant","content":"model: claimed, effort: high","effort":"high"}}),
        ] {
            claude_message(&mut reported, &event);
        }
        assert_eq!(
            serde_json::to_value(&reported).unwrap(),
            json!({
                "models":[], "effort":null, "truncated":false
            })
        );
        for event in [
            json!({"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant","model":"native-a","future":true}}}),
            json!({"type":"assistant","message":{"role":"assistant","model":"native-b"}}),
            json!({"type":"assistant","message":{"role":"assistant","model":"native-a"}}),
            json!({"type":"assistant","parent_tool_use_id":"child","message":{"role":"assistant","model":"child-model"}}),
        ] {
            claude_message(&mut reported, &event);
        }
        assert_eq!(reported.models, ["native-a", "native-b", "child-model"]);
        assert!(reported.effort.is_none());
    }

    #[test]
    fn provenance_bounds_unique_ids_without_shortening_them() {
        let mut reported = ReportedSettings::default();
        reported.model(&"é".repeat(128));
        reported.model(&"é".repeat(129));
        for index in 0..20 {
            reported.model(&format!("native-{index}"));
        }
        assert_eq!(reported.models.len(), 16);
        assert_eq!(reported.models[0], "é".repeat(128));
        assert_eq!(reported.models[15], "native-14");
        assert!(reported.truncated);
        let mut merged = ReportedSettings::default();
        merged.merge(reported);
        merged.model("native-14");
        assert_eq!(merged.models.len(), 16);
        assert!(merged.truncated);
    }
}
