# Task 8 报告：Web 收藏 API（5 个新端点）

## 实现内容

修改 `src/web/api.rs`（唯一改动文件）：

**路由**（`router()` 追加 4 条）：
- `GET/POST /api/favorites` → `list_favorites` / `add_favorite`
- `POST /api/favorites/remove` → `remove_favorite`
- `POST /api/favorites/install` → `install_favorite`
- `GET /api/targets` → `list_targets`

**5 个 handler**（追加在 `run_update` 之后，逐字采用 brief 代码）：
- `list_favorites`：registry 加载失败显式 500；成功原样序列化 `reg.favorites` map。
- `add_favorite`：`parse_source` 失败 → 400；`bookmark` 错误按类型分流——`Error::Msg|BadTarget`（用户输入类）→ 400，其余（clone/IO）→ 500；成功后落盘并返回 `{key, skills:n}`。
- `remove_favorite`：`resolve_key`/`unbookmark` 失败 → 404；成功落盘后返回 200。
- `install_favorite`：config 损坏显式 500（与 run_update 同一纪律，绝不回退默认配置）；`resolve_key` 失败 → 404；`Error::Conflict` 且无 `overwrite` → 409 + 路径明细，`overwrite:true` 时先按记录 `remove_install`（无记录则忽略）再重装；`Error::NotBookmarked` → 404；其余 → 500。成功返回 `{installed}`。
- `list_targets`：config 损坏显式 500；成功返回 `[{name, path}]`。

**use 区**：补 `Method`（`FavInstallReq.method` 需要）。

**测试**：追加 brief 给定的 4 个测试 + 2 个辅助函数（`make_local_source`、`post_json`）。

## 测试结果（RED/GREEN 证据）

**RED**（只加测试未实现时，`cargo test web::api`）：

```
failures:
    web::api::tests::add_favorite_rejects_bad_source_and_unknown_skill
    web::api::tests::favorites_api_lifecycle
    web::api::tests::install_favorite_conflict_then_overwrite
    web::api::tests::targets_endpoint_lists_configured
test result: FAILED. 7 passed; 4 failed
```

4 个新测试全部 404（路由不存在），与 brief 预期一致。

**GREEN**（实现后）：
- `cargo test web::api`：11 passed; 0 failed（7 既有 + 4 新增）
- `cargo test`（全量）：127（单元）+ 13（cli_smoke）+ 7（e2e）全绿
- `cargo clippy --all-targets`：零警告
- `cargo fmt --check`：通过

关键断言覆盖：收藏整仓返回 `key=local/mysrc, skills=2`；列表含技能描述快照；删单个/删整包/再删 404；坏 source 与未知技能 400；install 首次 200 → 冲突 409 → overwrite 重试 200 → 未收藏 404；targets 列表含 `agents`。

## 与 brief 的偏差

无实质偏差。测试与 handler 代码逐字采用 brief；仅 `cargo fmt` 做了纯格式化重排（闭包换行、`.map_err` 链式缩进），语义不变。

## 留给审查者的疑虑

1. **`add_favorite` 的 400/500 分流依赖错误类型约定**：`Error::Msg` 被整体归为「用户输入类 → 400」，但 `bookmark` 内部 `ensure_cached`/`scan_skills` 的某些 IO 失败若也包装成 `Msg`（如本地源路径不存在时的缓存拷贝失败），会被误报为 400 而非 500。这是 brief 的设计取舍（沿用了 CLI 的错误分类粒度），当前测试未覆盖该边界。
2. **`install_favorite` 的 overwrite 路径忽略 `remove_install` 错误**（`let _ =`）：与 CLI 覆盖路径语义一致（无记录则忽略），但若 remove 因 `Mismatch`（副本被外部改动）失败而 install 恰好不再冲突，理论上可能留下不一致现场。当前 copy 安装的存在性检查使该路径在实践中安全，测试已锁定 409→overwrite→200 链路。
3. **`list_targets` 暴露绝对路径**：绑 127.0.0.1 的本地 Web 契约需要路径展示给用户确认落盘位置，属设计意图；若未来绑定地址放开需重新评估。
