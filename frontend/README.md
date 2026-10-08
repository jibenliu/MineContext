# MineContext 前端

React / TypeScript 渲染层，桌面外壳为 Tauri，业务后端为 Rust daemon。

## 开发

在 `frontend/` 目录执行：

```bash
pnpm install --frozen-lockfile
npm run dev
```

纯浏览器开发使用本地开发替身，不代表真实采集或模型调用已验收。桌面运行与打包见 [运维指南](../docs/operations.md)。

## 命名约定

- `src/` 与 `packages/` 的文件及目录统一使用小写 `kebab-case`，例如 `chat-stream-service.ts`、`notification-provider.tsx`。
- 组件、类、枚举等 TypeScript 标识符仍采用 PascalCase，函数与 Hook 保持 camelCase；文件名规则不改变语言标识符惯例。
- 保留约定名称 `README.md`、`__tests__`、`__mocks__`，以及 `.page.test.tsx`、`.d.ts` 等用途后缀。
- 重命名须同步静态/动态导入、资源路径、测试和工具脚本；仅大小写变化需要 Git 明确记录，避免在 Linux CI 上失效。
- `npm run lint` 包含命名检查，并要求零 ESLint 警告；不通过关闭规则掩盖问题。

## 验证

共享业务状态使用 Redux。采集来源只在页面挂载或显式刷新时请求，并且不持久化、不参与跨窗口状态同步；请求失败可重试，旧响应不会覆盖最近的刷新结果。

```bash
npm run lint
npm run typecheck
npm test
npm run build:web
```

开发按改动面运行测试，不自动启动冒烟。提交前在仓库根运行 `./scripts/verify-affected.sh --with-smoke`；发版前使用 `./scripts/verify-all.sh --with-smoke`。

## 助手功能边界

正式助手支持流式回答、历史会话、复制、重新回答、停止生成、处理进度与本地引用展示。旧 `#/ai-demo` 地址转到正式助手，模型配置来自设置页，不使用硬编码演示端点。

处理进度来自后端事件，不伪造模型思维过程。重新回答会发起一次新的模型请求；停止会请求服务端中断并关闭前端流，不承诺供应商已经生成的 token 不计费。引用来自本地检索，并随消息保存；旧消息没有引用数据时不编造来源。联网搜索、工具执行、网页预览、回答分支不是已有后端契约，不展示假开关或空操作。
