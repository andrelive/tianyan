//! tools-todo 插件：会话绑定的推理辅助工具（`todo` / `goal`）。
//!
//! ## 为什么独立成 crate（ADR-049 期 1）
//!
//! 待办与目标是**智能体自主跟踪进度的工具**（不是纯 UI 功能）：agent 用 `todo`
//! 拆分任务、用 `goal` 跟踪长期目标，两者按 `session_id` 归属数据。它们的实现
//! 此前住在 server（`server/src/api/{todos,goals}/tool.rs`）——改一行工具描述
//! 要重编 server（13.4K 源码）。搬进本 crate 后只重编插件 + bin。
//!
//! ## 与 `tools-fs` / `tools-code` 的形态差异：store 由装配层传入
//!
//! 那两个插件的工具只需 [`ToolContext`]（路径解析 + 安全策略），本插件的工具
//! 还需**数据存储**：`TodoStore` / `GoalStore`。它们**留在 core**
//! （`tianyan::todos` / `tianyan::goals`）——是 store 不是工具，且 REST API、
//! 会话面板与工具必须读写**同一份**实例（工具写的待办要立刻出现在 UI 里）。
//!
//! 所以工厂签名不能只靠 `ToolContext`：装配层（`app/src/main.rs`、
//! `tauri/src/server.rs`）从 `AppState::todo_store()` / `goal_store()` 取
//! store 交给 [`tools`]。server 库因此仍不依赖本 crate（闸门 A 保持）。
//!
//! ## 会话绑定
//!
//! `execute(session_id, args)` 用 `session_id` 归属数据：创建时归当前会话，
//! list/update/delete 只作用于本会话条目（跨会话 id 一律 not-found，原子拒绝）。
//!
//! ## 展示意图
//!
//! 两个工具都用默认的 [`ToolPresentation::Generic`]（它们产出的是结构化状态，
//! 不是文件/搜索卡片）——与迁移前的渲染一致，故 `MIGRATED_TOOL_PRESENTATIONS`
//! 无需追加条目（该表只兜底非默认意图）。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use tianyan::agent::{DynamicToolExecutor, ToolContext};
use tianyan::common::error::Result;
use tianyan::goals::{GoalStatus, GoalStore};
use tianyan::model::types::{FunctionDefinition, ToolDefinition, ToolPresentation};
use tianyan::todos::{count_by_status, TodoDraft, TodoItem, TodoPriority, TodoStatus, TodoStore};
use tianyan::TianyanError;

// ── todo ─────────────────────────────────────────────────────────────────

/// 清单里的**一条**待办（整表替换语义：一次调用传完整计划）。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TodoDraftArgs {
    /// 标题（必填，非空）。写成**可独立验证完成**的最小单元。
    pub title: String,
    /// 详细描述（可选；进上下文时压成单行并截断到 60 字符）。
    #[serde(default)]
    pub description: Option<String>,
    /// 状态（缺省 pending）：pending | in_progress | completed。
    #[serde(default)]
    pub status: TodoStatus,
    /// 优先级（缺省 medium）：low | medium | high。
    #[serde(default)]
    pub priority: TodoPriority,
    /// 关联目标 id（可选；目标进度按关联待办的完成比例计算）。
    #[serde(default)]
    pub goal_id: Option<String>,
}

/// todo 工具参数——**只有一个字段**。
///
/// 此前的 `operation`（create/update/list/delete/close 分派）+ 5 个可选数组
/// 字段，是模型参数报错的主因：`operation` 必填且无 `serde(default)`，模型漏传
/// 就直接 `missing field 'operation'`（实测报错原文）。参考实现（OpenCode
/// `todowrite` / DSH `dsh-tool-todo`）的共识是**只剩一个 `todos` 数组 + 整表
/// 替换**——结构上不可能漏必填字段，同一份意图也只有一种写法。
///
/// 三态语义（靠「有没有传」区分，而不是靠一个 operation 字符串）：
/// - `todos: [非空]` → 整表替换（id 重新生成是既有语义）；
/// - `todos: []` → 清空该会话待办；
/// - `todos` 缺省 / `null` → 只读取当前清单（**不写**）。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TodoArgs {
    /// 完整清单（**整表替换**：未包含的旧条目即被移除）。传 `[]` 清空该会话
    /// 待办；**不传则只读取当前清单**（不写入）。
    #[serde(default)]
    pub todos: Option<Vec<TodoDraftArgs>>,
}

/// `todo` 工具：让智能体在会话内维护待办清单（整表替换）。
///
/// 待办是智能体可调用的工具（非纯 UI 功能）：agent 自主拆分任务、跟踪进度、
/// 标记完成。会话页输入框上方展示待办，完成条目保留展示（划线样式），
/// 无任何条目后面板消失（对齐 DSH todo_write 的临时面板语义）。
///
/// **当前清单每轮自动注入上下文**（`Agent::assemble_context` →
/// [`tianyan::todos::render_todos_summary`]），所以模型不需要为了「想起来」
/// 而反复调用本工具读清单；本工具的职责是**写**（整表替换）与按需读取。
///
/// 批语义（同一会话同一时期只有一批同源待办，last-write-wins）：
/// - 传非空 `todos` = 「当前完整计划」，旧批（含已完成项）整体替换——未包含的
///   旧条目即被移除（被中止/放弃的任务随重规划自然消失）；想保留的条目
///   （包括已完成的）必须包含在新列表中。
/// - 传 `[]` = 清空当前批（目标变更、整批作废时使用）。
/// - 不传 = 只读取（不写）。
///
/// **已放弃的能力：`parent_id` 子任务挂靠**。整表替换下 id 每次重新生成，
/// 挂靠只能「先 create 再 update 挂靠」两步走——那正是报错来源之一。子任务
/// 属低频能力，砍掉换结构简单（前端面板的 `parent_id` 缩进展示对平铺清单
/// 自然退化为无缩进，不报错）。
pub struct TodoTool {
    store: Arc<TodoStore>,
}

impl TodoTool {
    /// 绑定待办存储（与 REST API 共享同一实例，由装配层传入）。
    pub fn new(store: Arc<TodoStore>) -> Self {
        Self { store }
    }

    fn todo_json(i: &TodoItem) -> serde_json::Value {
        // 状态/优先级经 serde 序列化（snake_case/lowercase），与 REST API 语义一致；
        // Debug 格式会产出 "inprogress" 这类与 parse 口径不符的值（历史 bug）
        serde_json::json!({
            "id": i.id,
            "title": i.title,
            "description": i.description,
            "status": i.status,
            "priority": i.priority,
            "goal_id": i.goal_id,
        })
    }

    /// 统一返回形状：`{ action, summary, counts, todos }`。
    ///
    /// `summary` 是人/模型都易读的一句话（参照 dsh-tool-todo）：模型读 JSON 时
    /// 先看到它就能确认「这次调用到底改成了什么」，不必自己数组计数。
    /// `counts` 与注入摘要的头部共用 [`count_by_status`] 的计数口径。
    fn build_result(items: &[TodoItem], action: &str, summary: String) -> serde_json::Value {
        let (pending, in_progress, completed) = count_by_status(items);
        serde_json::json!({
            "action": action,
            "summary": summary,
            "counts": {
                "pending": pending,
                "in_progress": in_progress,
                "completed": completed,
            },
            "todos": items.iter().map(Self::todo_json).collect::<Vec<_>>(),
        })
    }

    /// 「N 项待办（a 待办 / b 进行中 / c 完成）」——返回值摘要与清空提示共用。
    fn counts_phrase(items: &[TodoItem]) -> String {
        let (pending, in_progress, completed) = count_by_status(items);
        format!(
            "{} 项待办（{pending} 待办 / {in_progress} 进行中 / {completed} 完成）",
            items.len()
        )
    }
}

#[async_trait]
impl DynamicToolExecutor for TodoTool {
    fn tool_name(&self) -> String {
        "todo".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        // schema 由 struct 派生（from_schema）——单一事实源：字段/枚举/必填随
        // TodoArgs 自动同步。手写 json! 曾与 struct 漂移。
        //
        // 描述只讲**怎么用**（何时用/何时不用/颗粒度/完成即更新），不讲 create/
        // update/delete 的语义——旧描述 90% 篇幅在解释 operation 分派，模型仍
        // 漏传 operation（参数报错）且规划质量差。整表替换语义已让「分派」消失。
        //
        // **必须是单行**：工具目录（`docs/architecture/tool-catalog.md`）把描述
        // 原样放进 markdown 表格单元格，换行会把一行表格拆成多行（表格结构坏掉）。
        ToolDefinition::function(FunctionDefinition::from_schema::<TodoArgs>(
            "todo",
            "维护当前会话的待办清单（整表替换：每次传当前完整计划，未包含的旧条目即被移除；传 [] 清空；不传则只读取当前清单）。当前清单每轮自动注入你的上下文，不必为「想起来」而反复读取。\
             【何时用】任务 ≥3 步、步骤间有依赖、或用户明确要求先列计划。\
             【何时不用】单步任务、管理成本高于执行成本的琐碎事、纯问答/闲聊——不要为走流程而建清单。\
             【颗粒度】每项写成可独立验证完成的最小单元（「跑通 X 的 3 个用例」而不是「做测试」）。\
             【必须遵守】每完成一步立刻重发整表并把它标 completed——不要等全部做完再一起标；进行中的项同一时刻只留一个。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        // 显式声明与默认值一致：待办产出结构化状态，不走文件/搜索卡片。
        ToolPresentation::Generic
    }

    async fn execute(&self, session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        // 空参数（模型「只想看看清单」，`arguments` 为 ""）视同缺省 → 只读取。
        // `serde_json` 对空串报 EOF，不该因此让一次合法读取失败。
        let args: TodoArgs = if arguments.trim().is_empty() {
            TodoArgs { todos: None }
        } else {
            serde_json::from_str(arguments)
                .map_err(|e| TianyanError::Custom(format!("tool: todo 参数无效：{e}")))?
        };
        match args.todos {
            // ① 缺省 / null：只读取，不写
            None => {
                let items = self.store.list_by_session(session_id).await;
                Ok(Self::build_result(
                    &items,
                    "read",
                    format!("当前清单：{}", Self::counts_phrase(&items)),
                ))
            }
            // ② 空数组：清空该会话待办（`replace_many` 拒绝空数组，故走 delete_by_session）
            Some(drafts) if drafts.is_empty() => {
                let removed = self.store.delete_by_session(session_id).await;
                Ok(Self::build_result(
                    &[],
                    "cleared",
                    format!("已清空：移除 {removed} 项待办"),
                ))
            }
            // ③ 非空：整表替换（id 重新生成是既有语义）
            Some(drafts) => {
                let mut parsed = Vec::with_capacity(drafts.len());
                for d in drafts {
                    // 显式校验：`replace_many` 也会拒，但这里给模型的动作级提示
                    // （错的是「某一项忘了写标题」，不是整表语义）
                    if d.title.trim().is_empty() {
                        return Err(TianyanError::invalid_input(
                            "tool: todo 每一项待办都需要非空 title",
                        ));
                    }
                    parsed.push(TodoDraft {
                        title: d.title,
                        description: d.description,
                        status: Some(d.status),
                        priority: Some(d.priority),
                        goal_id: d.goal_id,
                        // 整表替换不支持 parent_id（能力已砍——见 TodoTool 文档）
                        parent_id: None,
                    });
                }
                let (created, replaced) = self.store.replace_many(session_id, parsed).await?;
                let mut summary = format!("已更新：{}", Self::counts_phrase(&created));
                if replaced > 0 {
                    summary.push_str(&format!("，替换旧批 {replaced} 项"));
                }
                Ok(Self::build_result(&created, "updated", summary))
            }
        }
    }
}

// ── goal ─────────────────────────────────────────────────────────────────

/// goal 工具参数。
///
/// 单一形态：`status` **只表示 update 的设置值**。此前它兼作 list 过滤
/// （同一字段两义：update 设置 / list 筛选）——已去掉 list 的过滤分支以消除
/// 歧义（目标数量少，模型可直接从返回列表里筛）。类型化后非法状态由 serde
/// 直接拒绝（不再靠运行时字符串解析）。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GoalArgs {
    /// 操作：create | update | list | delete。
    pub operation: String,
    /// 目标 ID（update/delete 必填）。
    pub id: Option<String>,
    /// 标题（create 必填；update 可选）。
    pub title: Option<String>,
    /// 详细描述（可选）。
    pub description: Option<String>,
    /// 新状态（update 可选）。
    pub status: Option<GoalStatus>,
}

/// `goal` 工具：让智能体在会话内创建/更新/跟踪长期目标。
///
/// 目标进度按关联待办完成比例自动计算（与 todolist 联动）——agent 用
/// `todo` 工具拆解任务、`goal` 工具跟踪目标，形成"目标 → 待办"闭环。
/// 数据与会话绑定：创建时归属当前会话，list/update/delete 只作用于
/// 本会话条目；会话页展示活跃目标，完成/删除后面板消失。
pub struct GoalTool {
    store: Arc<GoalStore>,
}

impl GoalTool {
    /// 绑定目标存储（与 REST API 共享同一实例，由装配层传入）。
    pub fn new(store: Arc<GoalStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl DynamicToolExecutor for GoalTool {
    fn tool_name(&self) -> String {
        "goal".to_string()
    }

    fn definition(&self) -> ToolDefinition {
        // schema 由 struct 派生（from_schema）——单一事实源
        ToolDefinition::function(FunctionDefinition::from_schema::<GoalArgs>(
            "goal",
            "管理当前会话的长期目标（会话绑定，仅本会话可见）：创建/更新/列出/删除目标。目标进度按关联待办完成比例自动计算。operation: create（title 必填）/ update（id + 可选字段，status 为设置的新状态）/ list（列出本会话全部目标）/ delete（id）。",
        ))
    }

    fn presentation(&self) -> ToolPresentation {
        // 显式声明与默认值一致：目标产出结构化状态，不走文件/搜索卡片。
        ToolPresentation::Generic
    }

    async fn execute(&self, session_id: &str, arguments: &str) -> Result<serde_json::Value> {
        let args: GoalArgs = serde_json::from_str(arguments)
            .map_err(|e| TianyanError::Custom(format!("tool: goal 参数无效：{e}")))?;
        match args.operation.as_str() {
            "create" => {
                let title = args.title.unwrap_or_default();
                if title.trim().is_empty() {
                    return Err(TianyanError::invalid_input("tool: goal create 需要 title"));
                }
                let goal = self
                    .store
                    .create(title, args.description, None, Some(session_id.to_string()))
                    .await?;
                Ok(serde_json::json!({
                    "status": "created",
                    "id": goal.id,
                    "title": goal.title,
                }))
            }
            "update" => {
                let id = args
                    .id
                    .ok_or_else(|| TianyanError::invalid_input("tool: goal update 需要 id"))?;
                // 归属校验：只允许操作本会话的目标
                self.owned_goal(session_id, &id).await?;
                // 已是 Option<GoalStatus>（serde 校验；非法值在参数解析阶段被拒）
                let status = args.status;
                let updated = self
                    .store
                    .update(&id, args.title, args.description, status, None)
                    .await?;
                match updated {
                    Some(goal) => Ok(serde_json::json!({
                        "status": "updated",
                        "id": goal.id,
                        "title": goal.title,
                        "goal_status": format!("{:?}", goal.status).to_lowercase(),
                    })),
                    None => Err(TianyanError::not_found(format!("tool: goal 不存在：{id}"))),
                }
            }
            "list" => {
                // 只列出本会话的目标（会话绑定数据源）；`status` 不再兼作过滤
                // （一词两用已消除：它只表示 update 的设置值）
                let goals = self.store.list_by_session(session_id).await;
                let filtered: Vec<_> = goals
                    .into_iter()
                    .map(|g| {
                        serde_json::json!({
                            "id": g.id,
                            "title": g.title,
                            "status": format!("{:?}", g.status).to_lowercase(),
                        })
                    })
                    .collect();
                Ok(serde_json::json!({ "goals": filtered, "count": filtered.len() }))
            }
            "delete" => {
                let id = args
                    .id
                    .ok_or_else(|| TianyanError::invalid_input("tool: goal delete 需要 id"))?;
                self.owned_goal(session_id, &id).await?;
                let removed = self.store.delete(&id).await?;
                if removed {
                    Ok(serde_json::json!({ "status": "deleted", "id": id }))
                } else {
                    Err(TianyanError::not_found(format!("tool: goal 不存在：{id}")))
                }
            }
            other => Err(TianyanError::invalid_input(format!(
                "tool: goal 未知操作：{other}"
            ))),
        }
    }
}

impl GoalTool {
    /// 校验目标归属当前会话（不存在或属于其他会话 → not_found）。
    async fn owned_goal(&self, session_id: &str, id: &str) -> Result<()> {
        let owned = self
            .store
            .list_by_session(session_id)
            .await
            .iter()
            .any(|g| g.id == id);
        if owned {
            Ok(())
        } else {
            Err(TianyanError::not_found(format!(
                "tool: goal 不存在或不属于当前会话：{id}"
            )))
        }
    }
}

// ── 插件清单 ─────────────────────────────────────────────────────────────

/// 本插件提供的全部工具（装配层一行注册用）。
///
/// **顺序即注册顺序**：`todo` → `goal`（与迁移前组件工具的注入顺序一致）——
/// 工具定义是提示词前缀的一部分，顺序抖动会白掉前缀缓存。
///
/// `todo` / `goal` 两个 store 由**装配层**从 `AppState` 取出后传入（工具与
/// REST API 必须读写同一份数据，工具不能自建 store）；这是本插件与
/// `tools-fs` / `tools-code`（只需 `ToolContext`）的工厂签名差异。
pub fn tools(todo: Arc<TodoStore>, goal: Arc<GoalStore>) -> Vec<Arc<dyn DynamicToolExecutor>> {
    vec![Arc::new(TodoTool::new(todo)), Arc::new(GoalTool::new(goal))]
}

/// 插件清单工厂：把 store 绑定进清单，返回与 `tools-fs` / `tools-code` 同形的
/// 工厂（`FnOnce(&ToolContext) -> Vec<...>`）——装配层可直接交给 server 的
/// `ToolPluginFactory` 契约。
///
/// 上下文参数未使用：这两个工具的数据归属来自 `session_id`（执行期传入）与
/// 装配层传入的 store，不需要 `ToolContext`（无路径解析/安全策略需求）。
pub fn plugin_factory(
    todo: Arc<TodoStore>,
    goal: Arc<GoalStore>,
) -> impl FnOnce(&ToolContext) -> Vec<Arc<dyn DynamicToolExecutor>> {
    move |_ctx| tools(todo, goal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo_tool() -> (TodoTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (TodoTool::new(Arc::new(TodoStore::new(dir.path()))), dir)
    }

    fn goal_tool() -> (GoalTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (GoalTool::new(Arc::new(GoalStore::new(dir.path()))), dir)
    }

    /// 迁移门禁：两个工具都在，且顺序固定 `todo` → `goal`（顺序即注册顺序，
    /// 抖动会白掉提示词前缀缓存）。
    #[test]
    fn test_tools_names_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let exposed = tools(
            Arc::new(TodoStore::new(dir.path())),
            Arc::new(GoalStore::new(dir.path())),
        );
        let names: Vec<String> = exposed.iter().map(|t| t.tool_name()).collect();
        assert_eq!(names, vec!["todo", "goal"]);
        // 展示意图与迁移前一致（Generic = 默认值，故 MIGRATED_TOOL_PRESENTATIONS
        // 无需为这两个工具追加条目）
        for tool in &exposed {
            assert_eq!(tool.presentation(), ToolPresentation::Generic);
        }
    }

    /// `plugin_factory` 与 `tools` 产出一致（装配层两条路径不能分叉）。
    #[test]
    fn test_plugin_factory_matches_tools() {
        let dir = tempfile::tempdir().unwrap();
        let todo = Arc::new(TodoStore::new(dir.path()));
        let goal = Arc::new(GoalStore::new(dir.path()));
        let ctx = tianyan::agent::ToolRegistry::new(tianyan::executor::SecurityPolicy::default())
            .context()
            .clone();
        let from_factory: Vec<String> = plugin_factory(todo.clone(), goal.clone())(&ctx)
            .iter()
            .map(|t| t.tool_name())
            .collect();
        let from_tools: Vec<String> = tools(todo, goal).iter().map(|t| t.tool_name()).collect();
        assert_eq!(from_factory, from_tools);
    }

    /// 新 API ①：`todos` **缺省 / null → 只读取**（不写入，store 内容逐字段不变）。
    ///
    /// 判别力：这条锁的是「读」的三态语义——只读路径若顺手写了 store（哪怕写成
    /// 等价内容），`updated_at`/id 会变，前端面板会闪一下、会话面板的完成时间
    /// 也会被刷新。
    #[tokio::test]
    async fn test_todo_read_only_when_todos_absent() {
        let (tool, _dir) = todo_tool();
        let created = tool
            .execute(
                "s-1",
                r#"{ "todos": [ { "title": "甲" }, { "title": "乙", "status": "completed" } ] }"#,
            )
            .await
            .unwrap();
        assert_eq!(
            created["action"], "updated",
            "写路径：非空 todos = 整表替换"
        );
        assert_eq!(
            created["summary"], "已更新：2 项待办（1 待办 / 0 进行中 / 1 完成）",
            "summary 要人/模型都易读：{}",
            created["summary"]
        );
        assert_eq!(created["counts"]["pending"], 1);
        assert_eq!(created["counts"]["in_progress"], 0);
        assert_eq!(created["counts"]["completed"], 1);

        let before = tool.store.list_by_session("s-1").await;
        assert_eq!(before.len(), 2);

        // 缺省 todos（空对象）
        let read = tool.execute("s-1", "{}").await.unwrap();
        assert_eq!(read["action"], "read");
        assert_eq!(read["todos"].as_array().unwrap().len(), 2);
        assert_eq!(read["counts"]["completed"], 1);
        // 显式 null 同义
        let read_null = tool.execute("s-1", r#"{ "todos": null }"#).await.unwrap();
        assert_eq!(read_null["action"], "read");
        assert_eq!(read_null["todos"].as_array().unwrap().len(), 2);

        // 只读不写：ID / 时间戳全不变（写路径必然重新生成 id 并刷新 updated_at）
        let after = tool.store.list_by_session("s-1").await;
        assert_eq!(after.len(), before.len());
        for (a, b) in before.iter().zip(after.iter()) {
            assert_eq!(a.id, b.id, "只读路径不得改写条目");
            assert_eq!(a.updated_at, b.updated_at, "只读路径不得刷新时间戳");
        }
    }

    /// 新 API ②③：非空 → 整表替换；`[]` → 清空该会话（且不影响其他会话）。
    #[tokio::test]
    async fn test_todo_replace_and_clear() {
        let (tool, _dir) = todo_tool();
        // 整表替换：第二次只传 1 条 → 旧批 2 条整体移除
        tool.execute(
            "s-1",
            r#"{ "todos": [ { "title": "旧1" }, { "title": "旧2" } ] }"#,
        )
        .await
        .unwrap();
        let replaced = tool
            .execute("s-1", r#"{ "todos": [ { "title": "新批" } ] }"#)
            .await
            .unwrap();
        assert_eq!(replaced["todos"].as_array().unwrap().len(), 1);
        assert!(
            replaced["summary"]
                .as_str()
                .unwrap()
                .contains("替换旧批 2 项"),
            "替换要说明清掉了多少旧条目：{}",
            replaced["summary"]
        );

        // 其他会话不受影响
        tool.execute("s-2", r#"{ "todos": [ { "title": "他会话" } ] }"#)
            .await
            .unwrap();

        // 空数组 = 清空（replace_many 拒绝空数组，此处必须走 delete_by_session）
        let cleared = tool.execute("s-1", r#"{ "todos": [] }"#).await.unwrap();
        assert_eq!(cleared["action"], "cleared");
        assert_eq!(cleared["todos"].as_array().unwrap().len(), 0);
        assert!(
            cleared["summary"].as_str().unwrap().contains("移除 1 项"),
            "{}",
            cleared["summary"]
        );
        assert!(tool.store.list_by_session("s-1").await.is_empty());
        assert_eq!(
            tool.store.list_by_session("s-2").await.len(),
            1,
            "清空只作用于本会话"
        );
    }

    /// 报错路径：`title` 为空 → `invalid_input`（语义谓词判定，不做字符串匹配）。
    #[tokio::test]
    async fn test_todo_empty_title_is_invalid_input() {
        let (tool, _dir) = todo_tool();
        let err = tool
            .execute("s-1", r#"{ "todos": [ { "title": "  " } ] }"#)
            .await
            .unwrap_err();
        assert!(err.is_invalid_input(), "{err}");
        // 失败不写：store 保持为空
        assert!(tool.store.list_by_session("s-1").await.is_empty());
    }

    /// 前向兼容（治「模型调工具经常报错」）：老参数形状（带 `operation`）不再
    /// 报 `missing field 'operation'`——未知字段被忽略，语义只由 `todos` 决定。
    ///
    /// 判别力：实测报错原文就是 `missing field 'operation'`。模型带着旧习惯
    /// 多传一个字段时必须仍然可用，否则「报错 → 重试 → 又报错」的恶性循环照旧。
    #[tokio::test]
    async fn test_todo_ignores_legacy_operation_field() {
        let (tool, _dir) = todo_tool();
        // 带 operation=create + 非空 todos → 仍按整表替换
        let created = tool
            .execute(
                "s-1",
                r#"{ "operation": "create", "todos": [ { "title": "x" } ] }"#,
            )
            .await
            .unwrap();
        assert_eq!(created["action"], "updated");
        // 带 operation=list 但不传 todos → 只读（不报 "未知操作"）
        let read = tool
            .execute("s-1", r#"{ "operation": "list" }"#)
            .await
            .unwrap();
        assert_eq!(read["action"], "read");
        assert_eq!(read["todos"].as_array().unwrap().len(), 1);
        // 完全没传任何参数（模型漏参）→ 也只读，不报错
        let empty = tool.execute("s-1", "").await.unwrap();
        assert_eq!(empty["action"], "read");
    }

    /// schema：**只有一个字段 `todos`**，且非必填（结构上不可能漏必填字段）。
    ///
    /// 判别力：`operation` / `updates` / `ids` / `status` / `goal_id` 任何一个
    /// 回到顶层，都意味着「同一份意图又有了两套写法」。
    #[test]
    fn test_todo_schema_has_single_optional_field() {
        let (tool, _dir) = todo_tool();
        let params = tool.definition().function.parameters;
        let props = params["properties"]
            .as_object()
            .expect("顶层必须有 properties");
        let keys: Vec<&String> = props.keys().collect();
        assert_eq!(keys, vec!["todos"], "顶层只能有一个字段：{params}");
        // 非必填：required 为空或缺席（模型不传 = 只读，不该被判参数无效）
        let required_empty = match params.get("required") {
            None => true,
            Some(v) => v.as_array().map(|a| a.is_empty()).unwrap_or(false),
        };
        assert!(
            required_empty,
            "todos 必须非必填（缺省 = 只读取）：{params}"
        );
        for gone in ["operation", "updates", "ids", "goal_id", "parent_id"] {
            assert!(
                props.get(gone).is_none(),
                "旧字段 {gone} 不得出现在顶层：{params}"
            );
        }
        // 条目内的 status/priority 缺省值来自 core 的 Default 派生
        let all = params.to_string();
        for needle in ["TodoDraftArgs", "TodoStatus", "TodoPriority"] {
            assert!(all.contains(needle), "schema 缺少 {needle}：{all}");
        }
    }

    /// 描述：≤300 字，且四件必须讲的事都在（何时用 / 何时不用 / 颗粒度 /
    /// 完成即更新 + 清空与读取的写法）。
    ///
    /// 判别力：旧描述 90% 篇幅在解释 create/update/delete 语义，模型仍不会规划；
    /// 这条锁住「描述讲用法、不讲分派」。
    #[test]
    fn test_todo_description_covers_planning_guidance() {
        let (tool, _dir) = todo_tool();
        let desc = tool.definition().function.description;
        assert!(
            desc.chars().count() <= 300,
            "描述须 ≤300 字（每轮进提示词）：{} 字",
            desc.chars().count()
        );
        // 工具目录把描述原样放进 markdown 表格单元格：换行会把一行拆成多行
        assert!(
            !desc.contains('\n'),
            "描述必须单行（多行会破坏 docs/architecture/tool-catalog.md 的表格）：{desc}"
        );
        for needle in [
            "整表替换",
            "何时用",
            "何时不用",
            "颗粒度",
            "可独立验证",
            "completed",
            "[]",
            "不传",
        ] {
            assert!(desc.contains(needle), "描述缺少「{needle}」：{desc}");
        }
        // 不再解释 operation 分派（治「规划质量差」的根因之一）
        for gone in ["operation", "updates", "ids", "close"] {
            assert!(!desc.contains(gone), "描述不该再讲分派语义 {gone}：{desc}");
        }
    }

    #[tokio::test]
    async fn test_goal_list_no_longer_filters_by_status() {
        // status 一词两用已消除：它只表示 update 的设置值，list 不再按其过滤
        let (t, _dir) = goal_tool();
        t.execute("s-1", "{ \"operation\": \"create\", \"title\": \"目标A\" }")
            .await
            .unwrap();
        let listed = t
            .execute(
                "s-1",
                "{ \"operation\": \"list\", \"status\": \"completed\" }",
            )
            .await
            .unwrap();
        assert_eq!(listed["count"], 1, "list 不再按 status 过滤");
    }

    #[tokio::test]
    async fn test_goal_invalid_status_rejected_by_serde() {
        // 类型化（GoalStatus）后非法状态在参数解析阶段就被拒（不再靠运行时解析）
        let (t, _dir) = goal_tool();
        let err = t
            .execute(
                "s-1",
                "{ \"operation\": \"update\", \"id\": \"x\", \"status\": \"done\" }",
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("参数无效"), "{err}");
    }

    #[test]
    fn test_goal_definition_schema_derives_from_struct() {
        // schema 由 struct 派生（from_schema → schemars::schema_for!）：
        // 顶层结构 + 嵌套类型的 definitions。
        let (t, _dir) = goal_tool();
        let def = t.definition();
        let params = &def.function.parameters;
        assert_eq!(params["type"], "object");
        assert_eq!(params["required"], serde_json::json!(["operation"]));
        // status 的枚举取值（active/completed/archived）来自 GoalStatus 的
        // JsonSchema 派生——schemars 把嵌套类型放进 definitions、属性用 $ref 引用
        let all = params.to_string();
        assert!(all.contains("GoalStatus"), "{all}");
        assert!(
            all.contains("active") && all.contains("archived"),
            "枚举取值应来自 GoalStatus 派生：{all}"
        );
    }
}
