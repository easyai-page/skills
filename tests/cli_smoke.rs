use assert_cmd::Command;

fn skills_cmd(home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("skills").unwrap();
    c.env("SKILLS_HOME", home); // 测试隔离：Layout::new 读此环境变量
    c
}

fn run(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    skills_cmd(home).args(args).output().unwrap()
}

/// 断言成功，返回 stdout。
fn ok(home: &std::path::Path, args: &[&str]) -> String {
    let out = run(home, args);
    assert!(
        out.status.success(),
        "{args:?} 应成功: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// 断言失败（非零退出），返回 stderr。
fn fail(home: &std::path::Path, args: &[&str]) -> String {
    let out = run(home, args);
    assert!(!out.status.success(), "{args:?} 应失败");
    String::from_utf8(out.stderr).unwrap()
}

/// 写入 registry fixture：local/src 包下 alpha（带 tag t1）与 beta 两个 copy 副本 @ global:agents。
fn write_registry_fixture(home: &std::path::Path) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(
        home.join("registry.json"),
        r#"{
  "version": 1,
  "sources": {
    "local/src": {
      "url": "",
      "commit": "c1",
      "fetched_at": "2026-08-20T00:00:00Z",
      "auto_update": null
    }
  },
  "installs": [
    {
      "skill": "alpha",
      "source": "local/src",
      "source_path": "skills/alpha",
      "target": {"kind": "global", "name": "agents"},
      "method": "copy",
      "commit": "c1",
      "tags": ["t1"],
      "auto_update": null,
      "installed_at": "2026-08-20T00:00:00Z"
    },
    {
      "skill": "beta",
      "source": "local/src",
      "source_path": "skills/beta",
      "target": {"kind": "global", "name": "agents"},
      "method": "copy",
      "commit": "c1",
      "tags": [],
      "auto_update": null,
      "installed_at": "2026-08-20T00:00:00Z"
    }
  ]
}"#,
    )
    .unwrap();
}

/// 造一个本地源目录：<tmp>/src/skills/alpha/SKILL.md，返回源绝对路径。
fn make_local_source(tmp: &std::path::Path) -> std::path::PathBuf {
    let src = tmp.join("src");
    std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
    std::fs::write(
        src.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: A\n---\n",
    )
    .unwrap();
    src
}

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
        // 按行匹配：行首 token 等于子命令名，避免 contains 的子串恒真问题
        let found = s.lines().any(|l| l.split_whitespace().next() == Some(cmd));
        assert!(found, "help 缺少 {cmd}");
    }
}

#[test]
fn list_on_empty_layout_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    ok(tmp.path(), &["list"]);
}

// ---- 审查发现 1：config 自愈与写入前校验 ----

#[test]
fn config_set_rejects_out_of_range_values_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    // 越界端口 / 非数字端口 / 非法 method：写入前被拒
    let err = fail(home, &["config", "set", "web.port", "99999"]);
    assert!(err.contains("web.port"), "{err}");
    let err = fail(home, &["config", "set", "web.port", "abc"]);
    assert!(err.contains("web.port"), "{err}");
    let err = fail(home, &["config", "set", "defaults.method", "foo"]);
    assert!(err.contains("defaults.method"), "{err}");
    // 未知键的类型破坏由整份配置解析校验兜住（targets 值必须是字符串）
    let err = fail(home, &["config", "set", "targets.x", "123"]);
    assert!(err.contains("拒绝写入"), "{err}");
    assert!(!home.join("config.toml").exists(), "非法值不得落盘");
    // 合法值正常写入，且写出的配置能被 Config::load 解析（list 不崩）
    ok(home, &["config", "set", "web.port", "9000"]);
    ok(home, &["config", "set", "defaults.method", "copy"]);
    assert_eq!(ok(home, &["config", "get", "web.port"]).trim(), "9000");
    ok(home, &["list"]);
}

#[test]
fn config_subcommand_survives_and_heals_corrupt_config() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    // 手工写入损坏配置：越界 port + 非法 method（两个键同时坏）
    std::fs::write(
        home.join("config.toml"),
        "[web]\nport = 99999\n[defaults]\nmethod = \"foo\"\n",
    )
    .unwrap();
    // 依赖 Config::load 的命令失败……
    fail(home, &["list"]);
    // ……但 config 子命令仍可用（不依赖 Config::load）
    assert_eq!(ok(home, &["config", "get", "web.port"]).trim(), "99999");
    // 自愈路径：逐个键修回来。修第一个时另一个仍坏，写入必须被允许
    ok(home, &["config", "set", "web.port", "7823"]);
    fail(home, &["list"]); // method 仍坏
    ok(home, &["config", "set", "defaults.method", "symlink"]);
    ok(home, &["list"]); // 全部修复，CLI 恢复
}

// ---- 审查发现 2：add 的 -g 真实语义 ----

#[test]
fn add_global_flag_uses_first_configured_target_and_bare_add_uses_project() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let agents = tmp.path().join("g/agents");
    // 把内置 agents target 重定向到隔离目录，避免写真实 HOME
    ok(
        &home,
        &[
            "config",
            "targets",
            "add",
            "agents",
            agents.to_str().unwrap(),
        ],
    );
    let src = make_local_source(tmp.path());
    // -g 且无显式 --target：装进配置里第一个可用 global target（此处即 agents）
    let out = run(&home, &["add", src.to_str().unwrap(), "-s", "alpha", "-g"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(agents.join("alpha").exists());
    // 无 -g 无 --target：默认装进当前项目 <cwd>/.agents/skills
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let out = skills_cmd(&home)
        .args([
            "add",
            src.to_str().unwrap(),
            "-s",
            "alpha",
            "--method",
            "copy",
        ])
        .current_dir(&proj)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(proj.join(".agents/skills/alpha/SKILL.md").exists());
}

// ---- 审查发现 3：update 参数配对与多 skill ----

#[test]
fn update_requires_skill_and_target_together() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_registry_fixture(home);
    // skill 不带 --target：报错而非静默退化为全量
    let err = fail(home, &["update", "alpha"]);
    assert!(err.contains("--target"), "{err}");
    // --target 不带 skill：报错
    let err = fail(home, &["update", "--target", "global:agents"]);
    assert!(err.contains("技能"), "{err}");
    // --all 是显式全量，与技能名/--target 互斥
    fail(home, &["update", "--all", "alpha"]);
    fail(home, &["update", "--all", "--target", "global:agents"]);
    // 全量与 --all 正常工作
    ok(home, &["update", "--dry-run"]);
    ok(home, &["update", "--all", "--dry-run"]);
}

#[test]
fn update_multi_skill_with_target_selects_each() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_registry_fixture(home);
    // 多 skill + --target：逐个构造 selection，不再静默丢弃后续 skill
    let out = ok(
        home,
        &[
            "update",
            "alpha",
            "beta",
            "--target",
            "global:agents",
            "--dry-run",
        ],
    );
    assert!(out.contains("alpha"), "{out}");
    assert!(out.contains("beta"), "{out}");
    assert_eq!(out.matches("显式指定").count(), 2, "{out}");
    // 其中一个未安装：明确报错（dry-run 也不例外）
    let err = fail(
        home,
        &[
            "update",
            "alpha",
            "ghost",
            "--target",
            "global:agents",
            "--dry-run",
        ],
    );
    assert!(err.contains("未安装"), "{err}");
}

// ---- 审查发现 4：auto-update 三选一 ----

#[test]
fn auto_update_requires_exactly_one_policy_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_registry_fixture(home);
    let before = std::fs::read_to_string(home.join("registry.json")).unwrap();
    // 裸调：clap 拒绝，不得静默清设置
    fail(home, &["auto-update", "--source", "local/src"]);
    // 互斥：--on 与 --off 不能同给
    fail(
        home,
        &["auto-update", "--source", "local/src", "--on", "--off"],
    );
    let after = std::fs::read_to_string(home.join("registry.json")).unwrap();
    assert_eq!(before, after, "失败调用不得改 registry");
    // 正常三选一：on → inherit 清除
    ok(home, &["auto-update", "--source", "local/src", "--on"]);
    let reg = std::fs::read_to_string(home.join("registry.json")).unwrap();
    assert!(reg.contains("\"auto_update\": true"), "{reg}");
    ok(home, &["auto-update", "--source", "local/src", "--inherit"]);
    let reg = std::fs::read_to_string(home.join("registry.json")).unwrap();
    assert!(!reg.contains("\"auto_update\": true"), "{reg}");
}

// ---- 次要：remove 去重与错误传播 ----

#[test]
fn remove_dedups_overlapping_selections() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_registry_fixture(home);
    // --tag t1 命中 alpha，显式 alpha 又命中一次：去重后只删一次（磁盘不存在 → 仅清记录）
    let out = ok(home, &["remove", "alpha", "--tag", "t1"]);
    assert_eq!(out.matches("仅清记录").count(), 1, "{out}");
    let reg = std::fs::read_to_string(home.join("registry.json")).unwrap();
    assert!(!reg.contains("alpha"), "{reg}");
    assert!(reg.contains("beta"), "{reg}");
}

#[test]
fn remove_propagates_errors_with_nonzero_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    write_registry_fixture(home);
    // 显式删除未安装的 skill@target：错误传播，非零退出（不再 eprintln 吞掉）
    let err = fail(home, &["remove", "ghost", "--target", "global:agents"]);
    assert!(err.contains("未安装"), "{err}");
}
