# PendingCrew 产品介绍：React + TypeScript

以 `2026-10-14-product-tour` 为对照重构。保留介绍内容、原 PendingCrew 样式、谱系色条、群头像、聊天布局、三层计划、独立文件夹、唯一人类收件箱与原有交互。旧版目录和浏览器状态键不删除。

## 开发与验证

需要 Node.js 22.12+（本次使用 Node 26）、npm，以及浏览器测试用的 Chrome / Chromium。依赖精确版本由 `package-lock.json` 固定，安装使用 `npm ci`。

```sh
npm ci
npm run dev
```

```sh
npm run verify
```

`verify` 执行严格 TypeScript 检查、三种构建、状态层测试和浏览器检查。浏览器检查自行启动随机本机端口，结束后关闭，不需要手动保留开发服务器。macOS 默认使用 `/Applications/Google Chrome.app`；可用 `CHROME_PATH` 指定 Chrome，其他环境可以先执行 `npx playwright install chromium`。`npm run format` 格式化源码，不改产品仓库其他文件。

## 官网接入的三个选项

### 1. React 官网直接嵌入

入口组件为 `PendingCrewTour`。`lib/` 产物外置 React 依赖，包含 ESM、CSS、source map 和 TypeScript 声明。官网使用 React 19；不要在宿主里再加载独立页面的 React bundle。

将本项目作为 workspace / 本地依赖接入官网，构建后使用：

也提供可直接安装的本地组件包（没有发布到 npm 注册表）：

```sh
npm install ./pendingname-pendingcrew-product-tour-0.1.0.tgz
```

```tsx
import { PendingCrewTour } from "@pendingname/pendingcrew-product-tour";
import "@pendingname/pendingcrew-product-tour/styles.css";

export function PendingCrewSection() {
  return (
    <PendingCrewTour
      mode="embedded"
      height={800}
      storageKey="pendingname-website-pendingcrew-tour-v2"
      migrateLegacy={false}
    />
  );
}
```

官网负责自己的导航与其他产品区块；`embedded` 只呈现 Mac 界面。高度可指定为数字或 CSS 长度，建议至少 530px。使用全宽区块，界面沿用原有桌面/窄屏/手机布局。当前响应断点依据视口宽度，多个小卡片并排时优先使用 iframe，而不是把完整 Mac 界面压入窄列。

样式全部限定在 `.pc-tour` 容器内，原颜色变量与拖动尺寸也只写入该容器。不同实例使用不同 `storageKey`；需要纯临时体验时设置 `persist={false}`。外部导航、按钮、字体、根 CSS 变量与另一实例不受影响。`examples/host.html` 使用构建后的组件库和两个 StrictMode 实例，是实际接入验证，不是截图。

SSR 官网中把该组件放在客户端交互区。初始渲染使用确定的介绍数据，浏览器状态在挂载后恢复；本次验证包含 StrictMode 客户端接入，没有声称已在 Next.js 等完整官网中部署或验证。

### 2. 任意官网使用 iframe

将 `dist/` 的全部文件放到网站静态目录，保持资产相对路径：

```html
<iframe
  src="/demos/pendingcrew/index.html?embed=1"
  title="PendingCrew 交互介绍"
  loading="lazy"
  style="width:100%;height:800px;border:0"
></iframe>
```

适合官网使用不同框架，或希望隔离尺寸与依赖的情况。`?embed=1` 隐藏介绍网页的外部导航与页脚。无需真实 AI 或业务服务端。

### 3. 本地直接打开

打开 `portable/index.html`。它仍然是 React 组件编译后的页面，使用 IIFE bundle，解决 `file://` 无法正常加载 ES 模块的问题；不是退回旧 HTML 拼接实现。该版本也支持 `?embed=1`。

## 源码职责

| 位置                                      | 职责                                                                                         |
| ----------------------------------------- | -------------------------------------------------------------------------------------------- |
| `src/PendingCrewTour.tsx`、`src/index.ts` | 可复用组件、官网导航、公开 props 与导出                                                      |
| `src/content.json`                        | PendingCrew 三个介绍章节的文案                                                               |
| `src/fixtures.json`                       | 产品介绍的初始计划、消息、文档和引用                                                       |
| `src/domain/types.ts`                     | 计划、需求、人类事项、会话、消息、视图和命令的类型                                           |
| `src/domain/reducer.ts`                   | 原子状态操作与不变量；错误不能部分写入                                                       |
| `src/domain/selectors.ts`                 | 同源计划、引用、分类与查询                                                                   |
| `src/state/`                              | React 状态接入、弹窗、持久化、旧格式迁移与恢复                                               |
| `src/components/`                         | Sidebar、Chat、Inspector、Workspace、Dialogs、FolderControls、ResizeHandle 等真实 React 组件 |
| `src/styles/pendingcrew.css`              | 限定作用域的原 PendingCrew 样式                                                              |

界面使用 JSX 和直接事件处理，没有 `innerHTML`、`dangerouslySetInnerHTML` 或 document 级业务事件代理。输入框保持 DOM 身份与受控草稿；历史位置按会话保存。文件夹仅改变分类，进展与 Todo 引用同一份 Plan，没有复制状态。

介绍章节的内部 ID 仍为 website/trip/research，用于保留旧内容引用和浏览器状态兼容；可见内容全为 PendingCrew 产品介绍，并未恢复三个旧业务示例。

## 状态兼容与产品边界

新状态格式为版本 2。相同浏览器来源下，可导入旧 `pendingcrew-product-tour-v1` 中的消息、草稿、计划修改、分类和自建机组，新键保存新版，旧键保留。浏览器按来源隔离 localStorage，`file://`、localhost 和未来官网之间不会自动共享；迁移不上传数据。网站接入时建议使用自己的 storageKey 并关闭旧版迁移。

无存储权限或保存数据结构损坏时仍可操作，以初始介绍内容恢复。新机组保留独立初始快照，所以重置只影响当前机组。预置内容仍是交互介绍，不连接真实 crewd，不调用模型，也不操作本地产品文件。

本项目仅包含独立设计目录的网页 Demo，没有修改 Swift/Rust 后端、安装或替换 PendingCrew，也没有推送或发布官网。源码、文档与验证记录纳入版本控制；依赖目录、构建产物与生成的交付压缩包保留在本地。

## 验证证据

`evidence/browser-verification.json`：原版 13 项验收在 React 版通过。`evidence/react-verification.json`：额外 11 项检查，包括原版几何/字体/气泡逐项对照、焦点与光标、滚动位置、输入转义、持久化、跨机组答复、StrictMode 官网接入、样式隔离和两个实例的独立操作。状态层另有 20 项测试。

截图：`original-baseline.png` 与 `react-baseline.png`，以及桌面、手机和 `website-embedding.png`。正式构建、类型检查与本地交互证据不等同于已部署官网或安装版 GUI 验收。

构建方式参考 [React TypeScript 文档](https://react.dev/learn/typescript) 和 [Vite 构建/库模式文档](https://vite.dev/guide/build)，样式与产品内容以本仓原前端和上一版介绍为准。

## 概览群文案更新

机组名称改为「给你介绍一下我这个产品：PendingCrew」，主会话为「概览群」。初始消息只有用户提供的三条机长发言；第一条支持行内粗体，第三条附上 GitHub 仓库链接。沟通范围和「仅@你」位于群名右侧，删除冗余的「全部消息」状态文字。

右侧展示三条原始 Todo，「可以点我试试看」打开真实的 Todo 详情。contentRevision 迁移替换旧介绍，保留访客新增消息、草稿及记录，发生编号冲突时保持访客原引用。

新增 overview-verification.json 的 5 项检查；当前合计 51 项浏览器检查、20 项状态测试通过。视觉对照只要求未被这次明确修改的框架、列宽和原样式保持一致。

## 产品功能介绍与原生控件

文档与进展用 PendingCrew 自身的功能介绍产品，不再使用新手阅读任务。进展保持同源 Plan：最多三层、逐层折叠、全部展开与收起、详情编辑状态/负责人/备注，并关联需求、依赖、卡点和产物。三组文档覆盖持续协作、Todo 与进展、机组与聊天、独立分类及人的判断。

输入区对照 Sources/Chat/Vendored/ComposerView.swift：38px 独立圆形加号与 Todo 模式按钮、22px 圆角胶囊、内嵌 30px 发送箭头、空输入禁用、更多操作菜单。Todo 模式发送后留在聊天，原文入账并附上引用。Todo 概览对照 CrewTodoPanel.swift：暖白页面、白色卡片、状态圆圈与编号单独成行、放大看入口。

网页 macOS 质感参考 [Desktop UI](https://github.com/andrejilderda/desktop-ui) 与 [macOS Big Sur Clone](https://github.com/SergioSediq/macos-big-sur-clone)。借鉴控件尺寸、细边框、轻阴影和局部半透明菜单的处理，具体布局与控件结构仍以本仓 SwiftUI 为准。Desktop UI 的仓库标记为 abandoned，本项目没有引入它作为依赖。

contentRevision 3 更新预置文档与计划介绍，保留访客新增文档、草稿、计划修改和分类；新增文档发生编号冲突时保留访客编号。feature-verification.json 覆盖功能介绍、折叠、同源编辑、Todo 模式与移动端。

## 全 Demo 产品文案统一

预置成员为协作员，侧栏的阅读入口改为协作网络，开始使用章节改为组织与沟通。网站导航、机组简介、单聊、各群消息与文件夹均围绕 PendingCrew 自身功能；待人判断事项体现工作中的优先级与人的参与，取消阅读路线式选择。输入区不再显示发送键提示，键盘交互保持原样。

contentRevision 4 更新旧缓存的预置消息与名称，保留访客追加消息、草稿、分类和已答复状态。copy-verification.json 检查全部机组的三组会话及旧缓存恢复。

## 群专属工作空间

机组「介绍」包含总群、机长单聊、Todo介绍、演示策划、Demo开发。PendingCrew 下新增行政机组。会话列表常驻，取消会话分类，右侧使用 macOS 风格分段控件切换消息、Todo、文档与进展。

SceneState.db 保存当前群的工作账本，groups 保存其他群的工作数据；切换群时交换当前账本。消息与会话元数据保留在机组层，实际消息按会话 ID 隔离。稳定引用可以定位其他群，并切换到对应群。编号生成扫描所有群，新增 Todo 与文档只归属当前群；进展与 Todo 仍引用当前群的同一份计划。持久化同时校验并保存各群记录。

左侧色条保留 FNV 色相和父系分段，按本次要求把 HSV 调整为 S=.72、V=.96。会话选中改为淡灰。总群增加指定的人类发言；连续相同发送者只显示一次头像和名称，后续气泡直接跟随。右侧成员为机长、Todo介绍、人，删除 Todo 标题前的折叠标记，标题仍可折叠。

contentRevision 5 迁移旧机组共享记录到总群，保留访客内容和编辑，并为新群建立独立的工作数据。groups-verification.json 与 groups.test.ts 验证群间隔离、切换后恢复、稳定引用、唯一编号、手机显示和刷新持久化。框架视觉对照仅覆盖保留的外层窗口与侧栏，新群布局依据本次明确要求验收。

## 资料库：39 篇产品与设计文档

新增 src/documentLibrary.json，集中维护 39 篇面向产品理解的整理稿，来源为仓库 README、工作跟进设计及其裁定、全组织工作流、双向交接、文档/文件夹契约，以及后续团队数据、判断服务与远程配对草案。没有导入私人目录、账号凭据、机器配置、原始日志或验收流水。

分类：产品说明、已敲定设计、规划设想、工程说明。设计约定不表示所有安装版本均已交付；知识数据、语义内容块、自建表、版本和远程等内容保留相应草案状态。每篇有摘要、来源链接和相关文档引用。正文支持安全的标题、段落、粗体与列表；不执行文档 HTML。

分布：介绍/总群 9 篇、Todo介绍 6 篇、演示策划 4 篇、Demo开发 6 篇；协作方式/总群 4 篇、组织与沟通/总群 4 篇、行政/总群 6 篇。原有文档保留，其他群也保留各自说明。资料可以按文件夹筛选、通过相关文档跨群跳转，或者直接引用到当前群；发引用保留未发送草稿。

contentRevision 6 以 catalogKey 和稳定文档 ID 添加资料，保留现有内容、工作状态、选中的群、草稿和分类。编号冲突时保留访客原编号，重新分配预置资料的 ID，并在全组织范围解析相关引用。重置后再次解析跨机组引用。来源与标签字段进入保存状态校验。

验证：documentLibrary.test.ts 检查 39 篇资料、来源文件存在、相关引用可定位、群归属、编号冲突、迁移幂等和重置后的跨群引用；verify-library.cjs 的 6 项浏览器验收覆盖阅读/来源、跨群关系、文档分享、行政与专题资料、旧状态恢复和手机显示。

## 独立预览外壳

独立页面只保留产品窗口和重置按钮。删除外围品牌、网站导航、介绍与状态文字；窗口使用视口空间，机组切换统一走产品侧栏。嵌入组件继续只呈现产品窗口，宿主可自行组织官网导航。重置按钮保持可见，桌面与手机检查覆盖新的侧栏导航路径。

## 文档/数据与语义块示例

页面 title 与窗口品牌统一为 PendingCrew，使用 docs/app-icon.png 的原产品图标；public/app-icon.png 用于 favicon，src/assets/app-icon.png 由构建打入组件与便携包。机组名保留在标题栏上下文中。

「文档」入口改为「文档/数据」。资料库增至 45 篇/项：补充 Palantir 借鉴、当前 SQLite 方向、语义切分三篇说明，以及产品对象与关系、SQLite 产品事项表、共享内容块组合三个可操作数据视图。

出处核对：产品规格 §29 明确对象/关系/动作/视图与私人团队定位；当前 crewd/crates/store/src/sqlite_knowledge.rs 和 proto/src/sqlite_data_api.rs 展示每机组 SQLite 知识 API，早期 DoltLite 公共契约作为历史方案保留，不当作当前实现。team_data.rs 提供内容块、组合视图与字段类型依据。没有修改这些产品源码。

数据表支持搜索、列排序、选择行与关联资料；SQLite 表结构仅为示意，不执行 SQL。组合视图具有内容块 ID 和独立 placement ID，两份视图复用同一个内容块，编辑通过 saveDataView 仅写入网页文档记录与浏览器状态，不改变 Work/owner/完成状态。数据视图采用闭集类型和恢复校验，JSON/HTML/SQL 均不执行。

contentRevision 7 添加新资料，保留现有视图编辑与草稿；旧版本文档迁移先处理原始文档，再全局分配资料 ID，避免新资料编号与访客旧编号相撞。新增 dataViews.test.ts 两项检查与 verify-data.cjs 五项浏览器验收，覆盖品牌图标、对象关系表、字段/NULL、共享块保存/刷新和手机宽度。
