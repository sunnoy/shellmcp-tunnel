use shellmcp_agent::store::{Admission, Store};

#[test]
fn persistent_dedup_never_reexecutes_uncertain_or_completed() {
    let d = tempfile::tempdir().unwrap();
    let mut s = Store::open(d.path()).unwrap();
    assert!(matches!(
        s.admit("id", "fingerprint").unwrap(),
        Admission::New
    ));
    drop(s);
    let mut s = Store::open(d.path()).unwrap();
    assert!(matches!(
        s.admit("id", "fingerprint").unwrap(),
        Admission::Uncertain
    ));
    assert!(s.admit("id", "changed").is_err());
    s.complete("id", &serde_json::json!({"saved":true}))
        .unwrap();
    drop(s);
    let mut s = Store::open(d.path()).unwrap();
    assert!(matches!(
        s.admit("id", "fingerprint").unwrap(),
        Admission::Saved(_)
    ));
}

#[test]
fn unicode_byte_pages_roundtrip_without_replacements() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("中文.txt");
    let expected = "你好Rust🌍";
    std::fs::write(&p, expected).unwrap();
    let mut offset = 0;
    let mut output = String::new();
    loop {
        let page = shellmcp_agent::tools::read_utf8(&p, offset, 1).unwrap();
        let text = page["content"].as_str().unwrap();
        if text.is_empty() {
            break;
        }
        output.push_str(text);
        offset = page["next_offset"].as_u64().unwrap();
    }
    assert_eq!(output, expected);
}

#[test]
fn catalogue_limits_agree_with_execution() {
    let a = serde_json::json!({"command":"true","timeout_s":30});
    assert!(shellmcp_agent::catalogue::validate("shell_exec", &a).is_ok());
    assert!(
        shellmcp_agent::catalogue::validate(
            "shell_exec",
            &serde_json::json!({"command":"true","timeout_s":31})
        )
        .is_err()
    );
    assert!(
        shellmcp_agent::catalogue::validate(
            "job_output",
            &serde_json::json!({"job_id":"id","limit":0})
        )
        .is_err()
    );
}
