# enhance-kimi-web-search-moonshot

## 状态

**提案已完成内部审核修订（2026-07-17）。**  
审核决议已写入 `design.md`「审核决议」；实现前仍须人工确认该节后勾选 `tasks.md` §0。

## 目标（一句话）

当会话主模型为 Kimi 时，将本地 `web_search`（含可选 `web.run`）执行后端从强制依赖 OpenAI `alpha/search`，改为对齐 kimi-code 的 **Moonshot 官方简单搜索**（`POST …/search` + `text_query` → title/url/snippet），使纯 Kimi 环境具备可用查网能力。

## 文档地图

| 文件 | 内容 |
| --- | --- |
| [proposal.md](./proposal.md) | Why / What / Capabilities / Impact |
| [design.md](./design.md) | 架构、**已锁定**决策、鉴权、分期、风险 |
| [tasks.md](./tasks.md) | 可勾选实施清单（含审核门禁 §0） |
| [specs/kimi-moonshot-web-search/spec.md](./specs/kimi-moonshot-web-search/spec.md) | Kimi 路径 Moonshot 搜索行为需求 |

## 外部参考（行为对齐，不引入依赖）

- `/Users/mt/code/mt-ai/aicodex/kimi-code/packages/agent-core-v2/src/app/auth/webSearch/`
  - `providers/moonshot-web-search.ts` — `POST` body `{ text_query }`、结果映射
  - `tools/web-search.ts` — 工具面与结果格式化
  - `webSearchService.ts` — 配置 / managed 派生 `{base}/search`
- kimi-code 文档：`docs/zh/configuration/config-files.md` 的 `[services.moonshot_search]`  
  （Codex 落点为根配置段 `[moonshot_search]`，见 design 决议，避免伪造不存在的 `services` 段）

## 审核决议摘要

| # | 决议 |
| --- | --- |
| 1 | Kimi 判定：共享 slug 启发式（见 design），**不**用「全体 Compatible」 |
| 2 | Phase A 鉴权：Bearer API key / 当前 provider auth；**不做** kimi-code OAuth 刷新栈 |
| 3 | 配置：根段 `[moonshot_search]`（`base_url` / `api_key` / `custom_headers` / `enabled`） |
| 4 | **仅** Kimi 主模型路径启用；**禁止** OpenAI `alpha/search` 失败后的全局降级 |
| 5 | 出现任一富命令（open/click/…）→ **整次调用** unsupported；`recency`/`domains` 忽略并注明 |
| 6 | `web.run` 与 plain `web_search` **共用** Moonshot 后端（若 standalone 开启） |
| 7 | Feature `kimi_moonshot_web_search` **默认 true**；`moonshot_search.enabled=false` 可退回 OpenAI 路径 |

## 校验

```bash
openspec validate enhance-kimi-web-search-moonshot --strict
openspec status --change enhance-kimi-web-search-moonshot
```
