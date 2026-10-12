use serde_json::Value;

#[test]
fn only_local_control_webviews_receive_command_permissions() {
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/local-controls.json")).unwrap();
    assert_eq!(capability["local"], true);
    assert!(capability.get("remote").is_none());
    assert!(
        capability.get("windows").is_none(),
        "Window grants would include remote children"
    );
    assert_eq!(
        capability["webviews"],
        serde_json::json!(["controller", "browser-toolbar"])
    );
    let permissions = capability["permissions"].as_array().unwrap();
    assert_eq!(permissions.len(), 6);
    for permission in permissions {
        assert!(matches!(
            permission.as_str().unwrap(),
            "core:event:allow-listen"
                | "core:event:allow-unlisten"
                | "allow-browser-snapshot"
                | "allow-open-browser"
                | "allow-navigate-browser"
                | "allow-browser-action"
        ));
    }
    let config: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(
        config["app"]["security"]["capabilities"],
        serde_json::json!(["local-controls"])
    );
    assert_eq!(config["app"]["withGlobalTauri"], false);
}
