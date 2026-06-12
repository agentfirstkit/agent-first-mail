# 代码结构重整：拆分超大文件

## Context

`agent-first-mail` 的若干源文件已膨胀到难以维护，最突出的是 `src/store/mod.rs`(6618 行)——单个 `impl Workspace` 块横跨 95–4343 行，混杂 config / case / message / triage / archive / remote-sync / 渲染 / 引用收集 8 类职责。其余偏大文件：`imap_pull.rs`(1887)、`config.rs`(1833)、`push_queue.rs`(1130)。

本次工作是**纯机械、行为保持**的模块拆分：把大文件按职责切成聚焦的子模块，结构清晰、单文件可控。**不改任何逻辑、签名、公共 API 路径或磁盘格式**，只动：函数体整段搬移、可见性 token(`fn`→`pub(super) fn`)、`mod` 声明、`pub(crate)/pub(super) use` 再导出。成功判据：每一步后 `cargo build && cargo test && cargo clippy` 全绿(基线：59 个单元测试通过、0 clippy 错误)。

范围：**所有较大文件都整理**，store 取**中等粒度**。

## 关键约束：Rust 可见性

`Workspace` 是单一类型，其 `impl` 可分散到多个子模块文件(Rust 合法，多 `impl` 块)。规则：

1. **`pub` 方法与类型路径稳定**——`runner.rs`/`pipe.rs` 调用的 `Workspace::*` 公共方法可自由搬到任意 `store/*.rs`，无需再导出。
2. **无修饰 `fn foo()` 私有于其所在子模块**——若 A 子模块定义、B 子模块调用，必须升为 `pub(super)`(对父模块 `store` 可见，即对所有兄弟子模块可见)。只在本子模块内调用的保持私有。不确定时一律 `pub(super)`(不越出 crate，安全)。
3. **外部路径必须稳定(最高风险)**——`lib.rs` 有 `pub mod store;`。当前可经 `crate::store::X` 触达的**自由函数**搬入子模块后，须在 `mod.rs` 用 `pub use sub::X;` 再导出。已核实的外部引用：
   - `now_rfc3339`(留在 mod.rs)、`clean_body_text`、`render_message_section`、`render_message_section_with_config`(均 `pub`，被 mail/smtp_send/imap_pull 调用 → render.rs 后再导出)
   - `render_triage_view`(`pub(crate)`，imap_pull 调用 → triage.rs 后再导出)
   - `Workspace::{ensure_archive_eligible, message_remote_locations, message_remote_locations_any, add_remote_flags, read_message_by_id, relocate_message, ensure_message_ids_unreferenced}`(`pub(crate)`，push_queue 调用；方法路径稳定，保持 `pub(crate)` 即可)

## 目标布局

```
src/store/
  mod.rs          # Workspace 结构体、刷新统计结构体、mod 声明、再导出块；
                  # 生命周期/编排：at/discover/root/init/status/purge/pull/
                  # reconcile_remote_missing/config_*/remote_*/push/push_list/
                  # render_refresh/render_templates/log_*；now_rfc3339；merge_*_into_pull
  util.rs         # 共享 fs/string/校验/uid/time/rfc822 自由助手 + 高频 Workspace 小方法
                  # (require_workspace/append_audit_event/checked_reason/ui_language/
                  #  message_path/message_date/next_case_uid/...)，全部 pub(super)
  cases.rs        # case CRUD + 解析/枚举 + 归档case操作 + notes + items；ArchivedCaseEntry
  messages.rs     # 处置(ignore/spam/unspam/archive/trash/untrash/unarchive) + 消息读取/
                  # relocate + related/thread + update_messages_workspace + move_to_deleted
  disposition_views.rs # spam/trashed/deleted_remote 根目录生成视图(status/index.md.j2 + status/message.md.j2)
  purge.rs        # 本地永久清理旧 spam/trashed/deleted_remote 消息记录 + 刷新 disposition views
  archive.rs      # 直邮归档类目(create/show/restore/move/rename/summary/notes + dir/io/索引渲染)
  triage.rs       # triage 视图 + case 消息视图渲染(derived_status/triage_candidate/
                  # write_triage_view/render_case_index/render_case_message_view/...)
  drafts.rs       # 草稿(validate/remove/compose/reply/create + 附件 fetch + 草稿状态)；
                  # DraftValidation/DraftStateFile/DraftStateEntry
  render.rs       # 消息渲染自由函数 + thread 助手 + 模板助手(clean_body_text/
                  # render_message_section*/message_section_context/render_template/...)
  remote_sync.rs  # 远端位置 reconcile + 引用收集(collect_*_references/ensure_archive_eligible/
                  # message_remote_locations*/add_remote_flags/active_remote_locations/...)
  refs.rs         # 既有，CaseIndex，零改动
  tests.rs        # 由 mod.rs 内联 mod tests 搬出，#[cfg(test)] mod tests;

src/push_queue/
  mod.rs          # 全部 pub 函数(queue_*/push/list/mode_summary/remove_*) + 枚举/结构体 + preview_hint
  execute.rs      # push_outbound_*/push_action_steps/execute_*/push_special_use_move/
                  # resolve_action_mailbox_folder/mailbox_is_kind(入口 pub(super))
  preview.rs      # filtered_items/actions_for/item_summary_label/item_has_move_to/step_label
  io.rs           # read/write/delete_item/push_path/read_item_eml/find_case_path*/unique_push_id/...

src/imap_pull/
  mod.rs          # 全部 pub 包装 + 共享 pub 类型(PullTarget/MoveOutcome/RemoteMessage/
                  # FolderUidSnapshot/MailboxInfo) + pull_workspace/resolve_pull_targets/
                  # remote_*/uid_*/append_*/fetch_uid_snapshots/...；内联 mod tests 保留在此
  session.rs      # 所有 *_session + login_*/fetch_*/list_*/capability_move/require_move/...
  special_use.rs  # resolve_special_use_from_mailboxes + special_use_*/fallback_names/...
  identity.rs     # RemoteIndex/SavedMessage/ImapKey/save_remote_message/stable_message_id/
                  # add_remote_location/rfc822_*/fnv1a_hex/normalize_message_id/...

src/config/
  mod.rs          # 全部 serde 类型 + impl MailConfig 的 load/validate/默认构造等
  access.rs       # impl MailConfig 第二块：key 读写分发(get_key/set_key/get_mailbox_key/
                  # set_mailbox_key/get_pull_mailbox_action_key/set_*/get_archive_action_key/...)
  defaults.rs     # serde default_* / 默认 mailbox/actions 配置
  validation.rs   # legacy config 拒绝、secret/step/id/timezone/language 校验与 parse 小助手
  tests.rs        # config 单元测试

src/types/
  mod.rs          # crate::types::* 稳定再导出，保持外部调用路径不变
  ids.rs          # MessageId/CaseUid/ArchiveUid/PushId 透明 newtype
  message.rs      # MessageFile、认证/方向/状态、远端位置与 workspace 状态
  case_archive.rs # CaseMessages、ArchiveMessages、ArchiveMessageItem
  push.rs         # PushItem、PushPayload、PushStep*、MessageActionPush/OutboundPush
```

> config 保持**低风险切分**：类型、`Default` impl 与主 `impl MailConfig` 留在 `config/mod.rs`；key get/set 分发在 `config/access.rs`；serde 默认值、校验/parse 小助手与测试分别拆到 `defaults.rs`、`validation.rs`、`tests.rs`。`mod.rs` 通过私有 `use` 维持 serde default 路径和兄弟模块调用，避免改变外部 API。

## 逐项映射(职责桶)

完整 item→文件映射与每项可见性按上述桶归位。要点：

- **util.rs**：`read_to_string`/`write_string*`/`create_dir_all`/`read_dir`/`read_message`/`read_case_messages`/`case_status`/`message_json_paths`/`parse_*_ref`/`validate_*`/`*_dir_name`/`human_slug`/`message_time*`/`time_context`/`normalize_rfc822_message_id`/`audit_target`/`json_contains_any_id`/`merge_flags`/`ensure_no_name_conflicts`/`move_children` 等自由函数 + 高频方法 `require_workspace`/`append_audit_event`/`read_audit_events`/`checked_reason`/`ui_language`/`message_path`/`message_date`/`first_related_message_date`/`next_case_uid`/`next_archive_uid`，**全部 `pub(super)`**。`mod.rs` 加 `pub(super) use util::{case_status, read_case_messages};` 供 refs.rs 的 `super::` 导入解析。
- **cases.rs**：解析/枚举/notes/items 类(`resolve_active_case`/`find_case_by_uid`/`case_entries`/`all_case_entries`/`archived_case_entries`/`active_case_items`/`archive_case_items`/`notes_*`/`find_archived_case_by_uid`/`remove_empty_case_container_dir`)标 `pub(super)`；`ArchivedCaseEntry` 移此并 `pub(super)`。
- **messages.rs**：`read_message_by_id`/`relocate_message` 保 `pub(crate)`；`message_conversation*`/`related_message_ids`/`ensure_no_related_conversation`/`refresh_message*_after_ref_change`/`remove_triage_view_for_message`/`purge_message_artifacts` 标 `pub(super)`。
- **archive.rs**：`refresh_archive_indexes`/`refresh_archive_message_category_with_renderer`/`archive_message_category_ids`/`archive_message_category_items`/`find_archive_message_dir_by_uid` 标 `pub(super)`，其余私有。
- **triage.rs**：`render_triage_view` `pub(crate)` 再导出；`refresh_all_case_message_views`/`refresh_case_message_views`/`refresh_case_message_views_with_renderer` 标 `pub(super)`；case-index/view 渲染随 triage 同放(共用 thread 助手)。
- **render.rs**：3 个 `pub` 外部函数 + `render_message_section_with_options`/`message_section_context`/`message_template_value`/`markdown_inline`/`render_template`/`new_notes_md` + thread 助手标 `pub(super)`。
- **remote_sync.rs**：5 个 push_queue 用方法保 `pub(crate)`；`queue_archive_for_archived_messages`/`message_id_is_referenced` + 自由函数 `active_remote_locations`/`remote_location_missing`/`mark_remote_locations_missing`/`has_any_active_remote_location`/`add_queue_fields` 标 `pub(super)`；`collect_*_references`、`LocalRemoteLocation`/`ArchiveQueue`/`ArchiveEligibility`/`MailboxIdLocation` 等私有。
- **tests.rs**：保留 `#[cfg(test)]`(clippy 对 cfg(test) 豁免 unwrap/expect，零回归)。因 `store::tests` 是兄弟模块，`use super::*` 取不到桶内 `pub(super)` 项，需显式 `use super::util::{...}; use super::render::clean_body_text;` 等——最终 import 清单由编译收敛。
- **push_queue**：公共函数全留 mod.rs(免再导出)；execute/preview/io 入口标 `pub(super)`。
- **imap_pull**：pub 包装与共享 pub 类型留 mod.rs；session/special_use/identity 入口标 `pub(super)`；`save_remote_message`/`stable_message_id`/`resolve_special_use_from_mailboxes` 因测试引用标 `pub(super)`；**测试块留在 mod.rs 内联**避免兄弟可见性问题。

## 安全增量顺序(每步后 build+test+clippy 全绿，逐步提交)

1. `store/util.rs`(叶子助手，解锁其余；加 `pub(super) use util::{case_status, read_case_messages};`)
2. `store/refs.rs`(仅验证 `super::` 解析，无搬移)
3. `store/render.rs`(加 `pub use render::{...}`；整 crate build 确认 mail/smtp/imap 外部调用仍链接)
4. `store/remote_sync.rs`
5. `store/triage.rs`(加 `pub(crate) use triage::render_triage_view;`)
6. `store/archive.rs`
7. `store/messages.rs`
8. `store/cases.rs`
9. `store/drafts.rs`
10. `store/tests.rs`(最后，跨所有桶；靠编译收敛 import)
11. `store/mod.rs` 收尾(应落到 ~600–900 行)
12. `push_queue/`：io → execute → preview → mod 瘦身
13. `imap_pull/`：session → special_use → identity → mod 瘦身(测试块留内联)
14. `config/`：抽 `access.rs`、`defaults.rs`、`validation.rs`、`tests.rs`；保持 `crate::config::*` 外部路径稳定
15. `types/`：按 ids/message/case_archive/push 拆 `src/types.rs`；`types/mod.rs` 再导出全部公开结构化类型，保持 `crate::types::*` 外部路径稳定

## 验证

- 每步：`cargo build && cargo test && cargo clippy`(在 `spores/agent-first-mail/` 下；host 直接 cargo 可用，e2e 由 `AFMAIL_E2E=1` 门控，本次不涉及 Docker)。
- 终检：追加 `cargo test --test cli_integration`(集成测试调用编译出的 `afmail` 二进制，确认端到端不变)。
- 因纯机械搬移，**测试全绿即等于成功**；clippy 严格(deny unwrap/expect/panic/print)，纯搬移不应触发新 lint。

## 风险与对策

1. **外部路径断裂(首要)**——所有搬走的 `pub`/`pub(crate)` **自由函数**从 mod.rs 再导出；方法靠类型路径稳定。整 crate build 兜底。
2. **跨子模块私有可见性**——不确定即 `pub(super)`；编译器 `function ... is private` 精确定位漏网。
3. **内联测试可见性**——`store::tests` 需显式 `super::<bucket>::<item>` 导入；imap_pull/config 测试块留内联，把该问题只留给 store 一次。
4. **无环**——所有桶向内依赖叶子 `util.rs` 与经 `pub(super)` 的 `Workspace` 方法；跨 `impl` 调用按类型解析、无 `use` 环；共享结构体各只一处。
5. **serde 路径耦合**——正是 config 不做完整拆分、只抽 access 的原因。
6. `yaml_double_quote` 疑似既有死代码(0 内部调用)——保持私有、**不删**(删除属行为相邻，可能翻转 lint)；render 步骤再 grep 确认。

## 待实现的关键文件

- `src/store/mod.rs`(主切分点)、`src/store/refs.rs`(既有范式)
- `src/push_queue.rs`、`src/imap_pull.rs`、`src/config/mod.rs`
- `src/lib.rs`(模块声明保持不变——store/push_queue/imap_pull/config 仍是各自的 `pub mod`，仅内部由文件变目录)
