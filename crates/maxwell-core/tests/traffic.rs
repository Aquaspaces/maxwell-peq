use maxwell_core::traffic::{CAPACITY, Traffic};
use serde_json::json;

#[test]
fn worker_lines_become_entries_in_order() {
    let traffic = Traffic::default();
    traffic.push(json!({"kind": "start", "operation": "read"}));
    traffic.push_line(r#"{"trace":"out","hex":"060600055a0200"}"#);
    traffic.push_line(r#"{"trace":"in","hex":"0704000505b"}"#);
    traffic.push_line("thread 'main' panicked");
    traffic.push_line("   ");
    let log = traffic.since(0);
    let kinds: Vec<_> = log["entries"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["start", "out", "in", "note"]);
    assert_eq!(log["entries"][1]["hex"], "060600055a0200");
    assert_eq!(log["next"], 4);
    assert_eq!(traffic.since(3)["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn keeps_only_the_newest_entries() {
    let traffic = Traffic::default();
    for _ in 0..CAPACITY + 5 {
        traffic.push(json!({"kind": "out", "hex": "06"}));
    }
    let log = traffic.since(0);
    assert_eq!(log["entries"].as_array().unwrap().len(), CAPACITY);
    assert_eq!(log["entries"][0]["seq"], 5);
    traffic.clear();
    assert_eq!(traffic.since(0)["entries"].as_array().unwrap().len(), 0);
    assert_eq!(traffic.since(0)["next"], CAPACITY as u64 + 5);
}
