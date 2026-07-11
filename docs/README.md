# Aether 文档

## 快速开始

- [开发环境搭建](./GETTING_STARTED_DEVELOPMENT.md) - 从零开始运行项目
- [Aether CLI 参考](./reference/cli.md) - 当前命令、参数和部署模式

## 架构与核心概念

- [系统架构](./concepts/architecture.md) - 六个默认进程、数据权威和可选能力
- [数据通路](./concepts/data-flow.md) - SHM 上行、命令下行与派生结果通路
- [Aether Data Processing](./concepts/data-processing.md) - 面向所有 IoT 行业的可选数据处理边界
- [Data Processing 数据通路](./concepts/data-processing-flow.md) - 从数据窗口到 Processor 和派生结果的完整路径
- [ADR-0009](./adr/0009-aether-data-processing.md) - Data Processing 的命名、数据权威、当前 API/sidecar 组合与未来进程保留名

## Data Processing 接入

- [Data Processor 接入指南](./guides/data-processors.md) - 声明行业任务并连接本地或远程处理器
- [Data Processing 契约参考](./reference/data-processing-contracts.md) - 请求、ProcessingFrame、派生结果和错误语义
- [Data Processing Codec](../crates/aether-data-processing/README.md) - 严格 v1 JSON DTO、转换与 RFC 8785 输入摘要
- [Data Processing JSON Schema](../contracts/data-processing/README.md) - 严格的 v1 机器可读传输契约
- [HTTP Data Processor](../extensions/http-data-processor/README.md) - 有界的本地/远程 `DataProcessor` HTTP 适配器
- [SQLite HistoryQuery](../extensions/sqlite-history-query/README.md) - 默认只读历史查询、区间聚合与单事务快照
- [HTTP HistoryQuery](../extensions/http-history-query/README.md) - 仅用于预对齐 `last/reject` 网格的可选适配器
- [Energy Data Processing 资产](../packs/energy/data-processing/README.md) - 默认关闭的负荷/PV 任务、未投运绑定与契约 fixtures
- [Data Processing Runtime 模板](../packs/energy/data-processing/runtime.example.yaml) - 合成的严格投运配置模板，不能原样使用
- [功率预测领域映射](./domain/power-forecasting.md) - AetherEMS 负荷/PV 预测与现有 Load-Forecasting 服务的接入方式
- [Load-Forecasting Processor](../integrations/load-forecasting/README.md) - 无反向读的 `/v1/process` Edge-Platform 适配实现
- [Load-Forecasting 部署](../integrations/load-forecasting/deploy/README.md) - production override、preflight 与上线门槛
- [实现记录](./plans/2026-07-11-aether-data-processing.md) - 文档先行的实现顺序、已落地基线与生产投运门槛

## API 文档

- [HTTP API 参考](./reference/http-api.md) - 当前认证、服务端点与 Data Processing v1 路由
- [旧版 API 汇编](./API_REFERENCE.md) - 历史端点说明，不是当前完整契约
- [WebSocket API](./websocket-rule-monitor-api.md) - 实时数据推送接口

## 配置说明

- [配置格式指南](./CONFIG_FORMAT_GUIDE.md) - YAML、CSV、JSON 配置规范

## 运维参考

- [运维日志](./operations-log.md) - 问题记录与解决方案

---

## 常用命令

```bash
# 在仓库根目录生成只读计划；默认路径与 Compose 的 ./data 挂载一致
aether --json setup

# 审阅计划后应用输出中的同一 plan ID
aether setup apply --plan-id <PLAN_ID>

# 先按部署指南构建或加载 aetherems:latest，再启动六进程组合
aether services start

# 检查系统状态
aether doctor

# 查看帮助
aether --help
```

## 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `AETHER_IO_URL` | Io 服务地址 | `http://localhost:6001` |
| `AETHER_AUTOMATION_URL` | Automation 服务地址 | `http://localhost:6002` |
| `AETHER_CONFIG_PATH` | 配置文件目录 | 源码 checkout 为 `./data/config`；安装后由 install context 指定 |
| `AETHER_DATA_PATH` | 数据文件目录 | 源码 checkout 为 `./data`；安装后由 install context 指定 |
