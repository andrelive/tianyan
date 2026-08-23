# 后端契约快照（contract fixtures）

本目录存放**真实后端**（`http://127.0.0.1:3000`）抓取的响应快照，供
`src/lib/__tests__/contract.test.ts` 验证前后端 DTO 契约（字段名 / 透传 /
往返稳定）。**快照已脱敏**：api_key 替换为 `REDACTED`，机器特定路径归一化。

## 刷新快照

1. 启动后端（`cargo run -p tianyan-server`，默认 3000）；
2. 运行：

```powershell
scripts/capture-contract-fixtures.ps1
```

3. 检查 git diff：**确认新增字段/改名是预期契约变更**，且 api_key 保持 REDACTED。

## 覆盖的端点

| 文件          | 端点                      | 用途                         |
| ------------- | ------------------------- | ---------------------------- |
| config.json   | GET /api/v1/config        | 配置 DTO（字段名/透传/往返） |
| models.json   | GET /api/v1/config/models | 模型目录                     |
| sessions.json | GET /api/v1/sessions      | 会话列表 DTO                 |
