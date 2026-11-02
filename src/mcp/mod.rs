//! Experimental MCP adapter over the shared resident session host.

mod push_output;
mod transport;

use serde_json::{Value, json};

pub use transport::execute;

const ITEM_METHOD: &str = "notifications/due/session_item";

fn open_result(mut value: Value) -> Value {
    value["stream"]["method"] = json!(ITEM_METHOD);
    value
}

fn session_item(mut item: Value) -> Value {
    if item["kind"] == "session_opened" {
        item["data"] = open_result(item["data"].take());
    }
    item
}

fn session_read_result(mut value: Value) -> Value {
    if let Some(items) = value["items"].as_array_mut() {
        for item in items {
            *item = session_item(item.take());
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_metadata_agrees_across_response_push_and_replay() {
        let opened = json!({"session_id":"s","stream":{"enabled":true}});
        let item = json!({"session_id":"s","cursor":1,"kind":"session_opened","data":opened});
        let direct = open_result(opened.clone());
        let pushed = session_item(item.clone());
        let replay = session_read_result(json!({"items":[item]}));
        assert_eq!(direct["stream"]["method"], ITEM_METHOD);
        assert_eq!(direct, pushed["data"]);
        assert_eq!(direct, replay["items"][0]["data"]);
        assert!(opened["stream"].get("method").is_none());
    }
}
