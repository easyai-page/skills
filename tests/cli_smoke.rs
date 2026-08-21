use assert_cmd::Command;

#[test]
fn help_lists_all_subcommands() {
    let out = Command::cargo_bin("skills")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    for cmd in [
        "add",
        "list",
        "remove",
        "update",
        "tag",
        "auto-update",
        "config",
        "tui",
        "ui",
    ] {
        assert!(s.contains(cmd), "help 缺少 {cmd}");
    }
}

#[test]
fn list_on_empty_layout_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("skills")
        .unwrap()
        .env("SKILLS_HOME", tmp.path()) // 测试隔离：Layout::new 读此环境变量
        .args(["list"])
        .output()
        .unwrap();
    assert!(out.status.success());
}
