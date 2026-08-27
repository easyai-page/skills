// 端到端集成测试：通过真实编译出的 skills 二进制跑完整流程，fixture 为本地 bare 仓库，
// 全程不依赖网络。SKILLS_HOME 指向临时目录实现隔离（Layout::new 优先读该环境变量）。
use assert_cmd::Command;
use std::path::Path;
use std::process::Command as P;

fn git(dir: &Path, args: &[&str]) {
    assert!(
        P::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success(),
        "git {:?}",
        args
    );
}

/// 造一个含两个技能的技能包 bare 仓库，返回 (bare 路径, work 路径)
fn fixture_repo(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let work = base.join("work");
    let bare = base.join("bare.git");
    for s in ["alpha", "beta"] {
        std::fs::create_dir_all(work.join(format!("skills/{s}"))).unwrap();
        std::fs::write(
            work.join(format!("skills/{s}/SKILL.md")),
            format!("---\nname: {s}\ndescription: 技能 {s}\n---\n# {s}\n"),
        )
        .unwrap();
    }
    git(&work, &["init", "-b", "main"]);
    git(&work, &["add", "."]);
    git(
        &work,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "c1",
        ],
    );
    git(&work, &["clone", "--bare", ".", bare.to_str().unwrap()]);
    (bare, work)
}

fn skills(home: &Path) -> Command {
    let mut c = Command::cargo_bin("skills").unwrap();
    c.env("SKILLS_HOME", home);
    c
}

/// 把内置 agents target 重定向到临时目录（Windows 路径需转义反斜杠），返回该目录。
fn redirect_agents_target(home: &Path) -> std::path::PathBuf {
    let agents_dir = home.join("agents-skills");
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!(
            "[targets]\nagents = \"{}\"\n",
            agents_dir.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    agents_dir
}

#[test]
fn add_list_remove_copy_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, _work) = fixture_repo(tmp.path());
    let agents_dir = redirect_agents_target(&home);
    // add：装 alpha 到 global:agents（copy）。裸 add 默认装项目 cwd，须显式 -t。
    skills(&home)
        .args([
            "add",
            &format!("file://{}", bare.display()),
            "-s",
            "alpha",
            "-t",
            "global:agents",
            "--method",
            "copy",
            "-y",
        ])
        .assert()
        .success();
    assert!(agents_dir.join("alpha/SKILL.md").exists());
    // 再 add 同仓库 → 复用缓存不重复下载（stdout 有提示）
    let out = skills(&home)
        .args([
            "add",
            &format!("file://{}", bare.display()),
            "-s",
            "beta",
            "-t",
            "global:agents",
            "--method",
            "copy",
            "-y",
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("已缓存"));
    // list
    let out = skills(&home).args(["list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("alpha") && stdout.contains("beta"));
    // remove（remove 无 -y 旗标，本就不交互）
    skills(&home).args(["remove", "alpha"]).assert().success();
    assert!(!agents_dir.join("alpha").exists());
    assert!(agents_dir.join("beta").exists());
}

#[test]
fn update_respects_two_level_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, work) = fixture_repo(tmp.path());
    let agents_dir = redirect_agents_target(&home);
    skills(&home)
        .args([
            "add",
            &format!("file://{}", bare.display()),
            "-s",
            "alpha",
            "-s",
            "beta",
            "-t",
            "global:agents",
            "--method",
            "copy",
            "-y",
        ])
        .assert()
        .success();
    // 包级开、alpha 副本关。
    // file:// URL 的 key 推导见 source.rs，不硬编码：从 registry.json 读真实 key。
    let reg_raw = std::fs::read_to_string(home.join("registry.json")).unwrap();
    let reg: serde_json::Value = serde_json::from_str(&reg_raw).unwrap();
    let key = reg["sources"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    skills(&home)
        .args(["auto-update", "--source", &key, "--on"])
        .assert()
        .success();
    skills(&home)
        .args(["auto-update", "alpha", "-t", "global:agents", "--off"])
        .assert()
        .success();
    // 仓库推新提交，alpha 改为 v2
    std::fs::write(
        work.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: v2\n---\n",
    )
    .unwrap();
    git(&work, &["add", "."]);
    git(
        &work,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "c2",
        ],
    );
    git(&work, &["push", bare.to_str().unwrap(), "main"]);
    // update：alpha 跳过，beta 更新
    skills(&home).args(["update"]).assert().success();
    let alpha_md = std::fs::read_to_string(agents_dir.join("alpha/SKILL.md")).unwrap();
    assert!(alpha_md.contains("技能 alpha"), "alpha 副本应被跳过");
    // 显式强制更新 alpha
    skills(&home)
        .args(["update", "alpha", "-t", "global:agents"])
        .assert()
        .success();
    let alpha_md = std::fs::read_to_string(agents_dir.join("alpha/SKILL.md")).unwrap();
    assert!(alpha_md.contains("v2"), "显式指定应强制更新");
}

/// symlink 方式全链路：安装产物是指向缓存的符号链接；remove 只删链接、缓存原样保留。
/// Windows 上 symlink 需开发者模式，失败时安装端会回退 junction（核心单测无覆盖，
/// e2e 不强行造权限条件），故本测试仅 unix。
#[cfg(unix)]
#[test]
fn add_list_remove_symlink_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, _work) = fixture_repo(tmp.path());
    let agents_dir = redirect_agents_target(&home);
    skills(&home)
        .args([
            "add",
            &format!("file://{}", bare.display()),
            "-s",
            "alpha",
            "-t",
            "global:agents",
            "--method",
            "symlink",
            "-y",
        ])
        .assert()
        .success();
    let link = agents_dir.join("alpha");
    // 从 registry 读真实缓存 key（key 推导规则见 source.rs，不硬编码）
    let reg_raw = std::fs::read_to_string(home.join("registry.json")).unwrap();
    let reg: serde_json::Value = serde_json::from_str(&reg_raw).unwrap();
    let key = reg["sources"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let cached_skill = home.join(&key).join("skills/alpha");
    // 安装产物是符号链接，且指向缓存内的技能目录；透过链接可读内容
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "symlink 方式安装的产物必须是符号链接"
    );
    assert_eq!(std::fs::read_link(&link).unwrap(), cached_skill);
    assert!(link.join("SKILL.md").exists());
    // list 能列出 symlink 安装
    let out = skills(&home).args(["list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("alpha") && stdout.contains("Symlink"),
        "{stdout}"
    );
    // remove 只删链接本身，缓存内容原样保留
    skills(&home).args(["remove", "alpha"]).assert().success();
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "remove 后链接应被删除"
    );
    assert!(
        cached_skill.join("SKILL.md").exists(),
        "remove symlink 安装不得触碰缓存"
    );
}

/// 收藏全链路：整仓收藏 → 两级列表 → 单技能删/补 → 从收藏安装 → 删收藏不影响已安装副本。
#[test]
fn favorites_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, _work) = fixture_repo(tmp.path());
    let agents_dir = redirect_agents_target(&home);
    let url = format!("file://{}", bare.display());

    // 收藏整仓
    let out = skills(&home).args(["fav", &url]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("已收藏") && stdout.contains("（2 个技能）"),
        "{stdout}"
    );

    // 两级列表
    let out = skills(&home).args(["fav"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("├─ alpha — 技能 alpha"), "{stdout}");
    assert!(stdout.contains("└─ beta — 技能 beta"), "{stdout}");

    // 从 registry 读真实 source key（file:// 的 key 推导见 source.rs，不硬编码）
    let key = {
        let reg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("registry.json")).unwrap())
                .unwrap();
        reg["favorites"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone()
    };

    // 删单个再补回（upsert）
    skills(&home)
        .args(["fav", "rm", &key, "--skill", "alpha"])
        .assert()
        .success();
    let out = skills(&home).args(["fav"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("alpha") && stdout.contains("beta"),
        "{stdout}"
    );
    skills(&home)
        .args(["fav", &url, "--skill", "alpha"])
        .assert()
        .success();

    // 从收藏安装：source 给 URL（验证 resolve_key 的规范化路径）
    skills(&home)
        .args([
            "fav",
            "install",
            &url,
            "--skill",
            "alpha",
            "-t",
            "global:agents",
            "--method",
            "copy",
            "-y",
        ])
        .assert()
        .success();
    assert!(agents_dir.join("alpha/SKILL.md").exists());

    // 收藏与安装是正交记录：删整包收藏，已安装副本原样保留
    skills(&home).args(["fav", "rm", &key]).assert().success();
    let out = skills(&home).args(["fav"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("（无收藏）"));
    assert!(
        agents_dir.join("alpha/SKILL.md").exists(),
        "删收藏不得影响已安装副本"
    );
    let out = skills(&home).args(["list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("alpha"));
}

/// 单技能仓库：二级留空，用途挂在一级行。
#[test]
fn favorites_single_skill_repo_display() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let work = tmp.path().join("solo-work");
    let bare = tmp.path().join("solo.git");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        work.join("SKILL.md"),
        "---\nname: solo\ndescription: 单技能用途\n---\n",
    )
    .unwrap();
    git(&work, &["init", "-b", "main"]);
    git(&work, &["add", "."]);
    git(
        &work,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "c1",
        ],
    );
    git(&work, &["clone", "--bare", ".", bare.to_str().unwrap()]);

    skills(&home)
        .args(["fav", &format!("file://{}", bare.display())])
        .assert()
        .success();
    let out = skills(&home).args(["fav"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("— 单技能用途"), "{stdout}");
    assert!(
        !stdout.contains("├─") && !stdout.contains("└─"),
        "单技能仓库不得有二级行: {stdout}"
    );
}

/// 缓存被手动删除后，fav install 自愈重克隆。
#[test]
fn fav_install_heals_missing_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, _work) = fixture_repo(tmp.path());
    let agents_dir = redirect_agents_target(&home);
    let url = format!("file://{}", bare.display());
    skills(&home).args(["fav", &url]).assert().success();
    let key = {
        let reg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("registry.json")).unwrap())
                .unwrap();
        reg["favorites"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone()
    };
    std::fs::remove_dir_all(home.join(&key)).unwrap();
    skills(&home)
        .args([
            "fav",
            "install",
            &key,
            "--skill",
            "beta",
            "-t",
            "global:agents",
            "--method",
            "copy",
            "-y",
        ])
        .assert()
        .success();
    assert!(agents_dir.join("beta/SKILL.md").exists());
}
