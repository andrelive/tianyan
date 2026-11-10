//! 技能摘要回填：把「长描述 | 适用场景: … | 来源: evolution」拼接摘要改写成
//! **一句话「场景 + 用途」**（一次性运维脚本）。
//!
//! 背景（2026-11-11 只读验证 `D:\tianyan` 真实库）：60 个技能的 abstract 是演化任务
//! 用模板拼出来的——`{description} | 适用场景: {全部场景} | 来源: evolution`，
//! 合计 **12,526 字符（均值 208 字，最长 782 字）≈ 6.6K tokens**。
//!
//! 而技能清单**全量常驻 system 前缀**（渐进式披露的发现环节，缺了模型无从知道
//! 有哪些方法论可调），所以这 6.6K 是**每轮都在付**的固定成本，占实测固定前缀
//! （约 16K tokens）的四成——其中被用过的只有 28 条。
//!
//! 试过并否掉的方案：读取侧按首句切 + 上限截断（现网 34/60 条被切在引号或括号
//! 中间，读起来比不列更难判断该用哪个）。**截断是把写入侧毛病转嫁给每一次请求**，
//! 正确的修法是让 abstract 本身就短。
//!
//! 写入侧已改（`apply_skill` 不再拼场景列表，并加超长告警）；本脚本把**存量**
//! 对齐到同一口径：
//!
//! 1. 每条技能改写为一句「什么场景用 + 它给什么」（目标 ≤ 60 字符）；
//! 2. 原有的「（来源: evolution）」标记**保留**（演化产物标识，且只占 13 字符）；
//! 3. 展开说明、步骤、反模式、全部适用场景都留在 L2 正文——那里按需加载才付成本。
//!
//! 改完摘要必须**重算 L0 向量**，否则检索仍按旧向量命中（`update_summary_vectors`）。
//! 幂等：已等于目标值的条目直接跳过，可重复运行。
//!
//! 用法：
//! ```text
//! cargo run -p tianyan-tools-ops --bin rewrite_skill_abstracts -- --dry-run   # 只读预览
//! cargo run -p tianyan-tools-ops --bin rewrite_skill_abstracts                 # 执行
//! ```
//!
//! **必须先停止天演服务**：本脚本按运维工具惯例直接打开生产库（`Database::open` +
//! 写回），运行中的服务持有 WAL 与内存缓存，双写会打架。

use std::path::PathBuf;

use tianyan::common::types::{ContentLevel, ContextNamespace, TianyanUri};
use tianyan::vfs::{ContentStore, VfsSearch, VirtualFileSystem};

/// 目标摘要表：`(技能 id, 一句话正文)`。
///
/// 正文写「什么场景用 + 它给什么」，不含来源标记（由脚本按原摘要决定是否追加）。
/// 措辞原则与规则 abstract 一致：**场景在前**（那是检索与人工判断的入口），
/// 动作/用途在后；不写「本文介绍…」「该指南说明…」这类元描述。
const REWRITES: &[(&str, &str)] = &[
    (
        "ai-diagram-source-first-review",
        "审 AI 制图：读 SVG 源建元素/连线事实表、脚本断言，禁止截图目测",
    ),
    (
        "app-shutdown-and-cancel-cleanup",
        "停止/退出失效：取消信号覆盖全入口 + 超时兜底 + 杀进程树",
    ),
    (
        "architecture-review-html-doc",
        "出架构/重构评审稿：单页 HTML + 每处设计写理由取舍 + 几何自检",
    ),
    (
        "bulk-text-migration-scripting",
        "多文件几十处机械替换：一次性脚本 + 唯一性断言，别逐条 apply_edit",
    ),
    (
        "change-verification-mutation",
        "行为改动验收：反向注入旧行为，测试必红以证明判别力",
    ),
    (
        "chat-history-stream-merge-consistency",
        "会话尾部重复/历史不加载/切回丢数据：按落库→订阅→归约分层取证",
    ),
    (
        "chat-scroll-position-recovery",
        "聊天列表上滚被拽回、切走跳位：显式跟随模式 + 锚点带偏移",
    ),
    (
        "claimed-action-verification",
        "质疑 AI 未执行/幻觉：副作用、落库、前端三层证据核验",
    ),
    (
        "code-review",
        "审查分支改动：Standards 与 Spec 双轴并行子代理审查",
    ),
    (
        "codebase-analysis-operation",
        "快速读懂大型代码库：规模扫描 + 符号大纲 + 分级问题报告",
    ),
    (
        "codebase-design",
        "设计模块接口：seam 判据、删除测试、接口即测试面",
    ),
    (
        "context-budget-accounting",
        "上下文暴涨/提前压缩：按 prompt 侧 token 拆构成定位主因",
    ),
    (
        "design-doc-code-fidelity-audit",
        "设计文档与代码对不上：按代码核实并批量修正",
    ),
    (
        "design-grill",
        "方案评审/拷问：逐分支追问到共识，一次一问给推荐答案",
    ),
    (
        "dev-lessons",
        "改动没生效、E2E 反复失败：先查环境跑的是哪份代码",
    ),
    (
        "diagnosing-bugs",
        "排查 bug、间歇故障：先造红得起来的反馈回路再谈假设",
    ),
    (
        "evolution-consumption-diagnostics",
        "演化写了没效果、技能零调用：查命中率判产出侧还是消费侧",
    ),
    (
        "frontend-theme-token-system",
        "硬编码色改主题令牌：分层 --app-*→--el-* + 计算色护栏验零差异",
    ),
    (
        "gnss-precip-forecast-design-review",
        "评审无雷达 GNSS AI 降水预测：先核标签与云图体积预算再谈模型",
    ),
    (
        "gnss-ztd-pwv-rapid-solving",
        "现网脉冲式 GNSS 解算 ZTD/PWV：体检→双差→质量治理→产品交付",
    ),
    (
        "handbook-language-audit",
        "审校使用者手册：口语改书面、删维护者指令、重建站点验收",
    ),
    (
        "handoff",
        "会话过长要交接：压成给下一个 agent 的交接件，只引用路径",
    ),
    (
        "llm-upstream-empty-response-probe",
        "自动化轮次出空响应：重建请求单因子对照 + 核上游源码",
    ),
    (
        "microkernel-plugin-migration",
        "workspace 按域拆编译期插件 crate，缩短增量编译",
    ),
    (
        "native-gui-automation-channel",
        "自研 GUI 让智能体验证界面：加 HTTP 动作接口 + 状态快照",
    ),
    (
        "native-gui-migration-feasibility",
        "评估 Tauri/Electron 迁原生 Rust GUI：先量化内存启动渲染再决策",
    ),
    (
        "oss-publication-history-sanitize",
        "开源前重写 git 历史：统一作者、脱敏路径与密钥、双端核查",
    ),
    (
        "parallel-agent-orchestration",
        "多 agent 同工作区并行：git 禁令、文件域分批、波次门禁",
    ),
    (
        "parallel-test-global-state-isolation",
        "并行测试串扰：test 侧隔离全局状态，不放宽生产语义",
    ),
    (
        "planning",
        "计划阶段软约束：只读研究 + 结构化计划 + 结束询问",
    ),
    (
        "quant-signal-research-and-backtest",
        "A 股策略信号回测：定义先行、统一超额口径、买卖风控成对重测",
    ),
    (
        "research",
        "调研主题/对标外部系统：派子代理、对一手来源、每结论引用出处",
    ),
    (
        "rime-weasel-input-method-ops",
        "部署维护 Rime 小狼毫：方案放 %APPDATA%\\Rime 后重部署并实测",
    ),
    (
        "role-codebase-explorer-guide",
        "何时委托 codebase-explorer 探索代码库、怎么组合步骤",
    ),
    (
        "role-grep-searcher-guide",
        "何时委托 grep-searcher 精确检索：pattern、glob、输出模式",
    ),
    (
        "role-rust-scan-script-author-guide",
        "委托 rust-scan-script-author 写风险扫描脚本（而非执行分析）",
    ),
    (
        "rust-best-practices",
        "写/审 Rust：借用克隆取舍、错误处理、性能心态、lint 纪律",
    ),
    (
        "rust-code-quality-scan",
        "Rust workspace 质量扫描：Clippy + 行数 + 风险模式",
    ),
    (
        "rust-workspace-test-operation",
        "Rust 测试执行：先 cargo check 再跑调试命令并判读输出",
    ),
    (
        "scientific-simulation-acceptance",
        "仿真项目验收：预注册判据、工具效度、阶梯运行、跨种子对照",
    ),
    (
        "search-backend-quality-diagnosis",
        "web_search 结果不相关：矩阵对照定位端点退化或限流",
    ),
    (
        "search-operation",
        "代码库结构化搜索：统计匹配、排除测试、技术债审计",
    ),
    (
        "simulation-metric-validity",
        "指标给出阴性结论前：先验证测量工具本身是否有效",
    ),
    (
        "snn-bottleneck-localization",
        "仿真行为不表达、成功率卡顶：三级分层量测定位真瓶颈",
    ),
    (
        "snn-circuit-debugging",
        "SNN 电路级调试：查连接矩阵静默故障、消融验因果",
    ),
    (
        "snn-gpu-port-and-verify",
        "CPU 仿真移植 GPU：同种子等价验证先于性能，须附设备证据",
    ),
    (
        "stakeholder-proposal-design-review",
        "组织内方案研讨稿：分离领导与用户想法、单页 HTML 交付",
    ),
    (
        "streaming-service-checkpoint-resume",
        "常驻流服务重启恢复：回放预热 + 实体水位线 checkpoint",
    ),
    (
        "svg-canvas-trim-for-office",
        "SVG 插 Office 白边过大：按真实包围盒裁 viewBox 保持矢量",
    ),
    (
        "tdd",
        "写新功能/修 bug 先写测试：红先于绿，测试写在预确认的 seam 上",
    ),
    (
        "technical-writing",
        "写/审技术文档：结论先行、断言可溯源、禁 AI 腔",
    ),
    (
        "third-party-installer-verification",
        "审计第三方安装包：双源哈希互证 + 验签 + MSI 静态审计",
    ),
    (
        "tianyan-release-build",
        "天演发新版：版本落点升级→门禁→构建→推 tag→判读 CI",
    ),
    (
        "tool-param-single-shape",
        "工具 schema 形态分叉：判别测试锁「旧形态必被拒」",
    ),
    (
        "upstream-reference-parity-audit",
        "与上游参考实现对账：先读源码核实前提再列缺口分级",
    ),
    (
        "vitepress-doc-diagram-and-verify",
        "VitePress 文档配图与验收：业务流程图 + Playwright 校验",
    ),
    (
        "windows-host-hardware-diagnosis",
        "Windows 卡顿/弹窗/疑似硬件故障：分层归因含厂商常驻层",
    ),
    (
        "windows-powershell-commanding",
        "天演 Windows 执行命令：后台任务、ANSI 卫生、参数陷阱",
    ),
    (
        "writing-skills",
        "给天演写技能：可预测性为根德、渐进披露、可检查完成判据",
    ),
    (
        "wsl-linux-replica-testing",
        "复刻 CI-only 失败：WSL 上命令逐字对齐 CI 并抗中断执行",
    ),
];

/// 来源标记（演化产物原有；只占 13 字符，保留以便区分预置技能）。
const SOURCE_TAG: &str = "（来源: evolution）";

#[derive(Default)]
struct Stats {
    scanned: usize,
    rewritten: usize,
    already: usize,
    missing: usize,
    over_limit: usize,
    chars_before: usize,
    chars_after: usize,
}

#[tokio::main]
async fn main() -> Result<(), tianyan::TianyanError> {
    let args: Vec<String> = std::env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let data_dir_override: Option<PathBuf> = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let config = tianyan_tools_ops::load_config(data_dir_override)?;
    tianyan_tools_ops::print_targets(&config);
    let vfs = tianyan_tools_ops::open_vfs(&config).await?;

    println!(
        "\n== 技能摘要回填（{}）==",
        if dry_run { "dry-run" } else { "执行" }
    );
    let mut stats = Stats::default();

    for (id, body) in REWRITES {
        let uri = TianyanUri::new(ContextNamespace::Skill, vec![id.to_string()]);
        stats.scanned += 1;

        let current = vfs
            .read_content(&uri, ContentLevel::Abstract)
            .await
            .unwrap_or_default();
        let current = current.trim();
        if current.is_empty() {
            stats.missing += 1;
            println!("  ⚠️ 技能不存在或摘要为空（跳过）：{id}");
            continue;
        }
        stats.chars_before += current.chars().count();

        // 来源标记随原摘要走：预置技能（planning）不带，演化产物保留。
        let new_abstract = if current.contains("来源: evolution") {
            format!("{body}{SOURCE_TAG}")
        } else {
            body.to_string()
        };
        stats.chars_after += new_abstract.chars().count();

        if current == new_abstract {
            stats.already += 1;
            continue;
        }
        if new_abstract.chars().count() > tianyan::skills::SKILL_ABSTRACT_MAX_CHARS {
            stats.over_limit += 1;
            println!(
                "  ⚠️ 目标超过 {} 字符（仍写入，但需人工收紧）：{id}",
                tianyan::skills::SKILL_ABSTRACT_MAX_CHARS
            );
        }

        println!(
            "  {id}\n    旧（{}字）: {current}\n    新（{}字）: {new_abstract}",
            current.chars().count(),
            new_abstract.chars().count()
        );

        if !dry_run {
            vfs.write_abstract(&uri, &new_abstract).await?;
            // 摘要变了必须重算 L0 向量：否则检索仍按旧向量命中，改摘要等于白改。
            // overview 取不到时用新摘要兜底（与 consolidate_rules / 规则回填同口径）。
            let overview = vfs
                .read_content(&uri, ContentLevel::Overview)
                .await
                .unwrap_or_else(|_| new_abstract.clone());
            if let Err(e) = vfs
                .update_summary_vectors(&uri, &new_abstract, &overview)
                .await
            {
                println!("    ⚠️ 向量更新失败（{e}），该条目 L0 向量仍是旧摘要");
            }
        }
        stats.rewritten += 1;
    }

    println!("\n== 汇总 ==");
    println!(
        "  扫描 {} / 改写 {} / 已达标 {} / 缺失 {} / 超限 {}",
        stats.scanned, stats.rewritten, stats.already, stats.missing, stats.over_limit
    );
    println!(
        "  摘要字符数 {} → {}（省 {}）",
        stats.chars_before,
        stats.chars_after,
        stats.chars_before.saturating_sub(stats.chars_after)
    );
    if dry_run {
        println!("  （dry-run：未写入）");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 目标摘要必须全部 ≤ 上限：超了就等于把「前缀每轮付费」的老问题带回来。
    ///
    /// 判别力：往 `REWRITES` 里加一条超长正文即变红。
    #[test]
    fn test_all_targets_within_limit() {
        for (id, body) in REWRITES {
            let with_tag = format!("{body}{SOURCE_TAG}");
            assert!(
                body.chars().count() <= tianyan::skills::SKILL_ABSTRACT_MAX_CHARS,
                "{id} 正文超限：{} 字",
                body.chars().count()
            );
            // 演化产物带来源标记，也必须在限内
            assert!(
                with_tag.chars().count() <= tianyan::skills::SKILL_ABSTRACT_MAX_CHARS + 13,
                "{id} 含来源标记后超限：{} 字",
                with_tag.chars().count()
            );
        }
    }

    /// 目标摘要不得写回演化拼接尾巴或元描述（那是这次要清掉的东西）。
    ///
    /// 判别力：把某条正文改回 `xxx | 适用场景: …` 或 `本文介绍…` 即变红。
    #[test]
    fn test_targets_have_no_meta_wording() {
        for (id, body) in REWRITES {
            assert!(!body.contains("| 适用场景:"), "{id} 仍含场景列表拼接");
            assert!(!body.contains("本文介绍"), "{id} 仍是元描述写法");
            assert!(!body.contains("该指南说明"), "{id} 仍是元描述写法");
            assert!(!body.contains("本指南用于"), "{id} 仍是元描述写法");
        }
    }

    /// 映射表不得有重复 id，且数量与现网技能数一致（漏一条 = 那条仍占着长摘要）。
    ///
    /// 判别力：删掉一条映射或重复一个 id 即变红。
    #[test]
    fn test_mapping_ids_are_unique_and_complete() {
        let mut ids: Vec<&str> = REWRITES.iter().map(|(id, _)| *id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "映射表有重复 id");
        assert_eq!(total, 60, "应覆盖现网 60 个技能，实际 {total}");
    }
}
