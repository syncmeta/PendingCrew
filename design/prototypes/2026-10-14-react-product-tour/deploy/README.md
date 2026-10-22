# pendingname.com/crewdemo

本目录保存 Demo 的独立 Cloudflare Workers Static Assets 部署配置。仅接管 `pendingname.com/crewdemo` 与 `/crewdemo/*`，不替换根站点 Worker，不修改 DNS，也不部署 `pending.name`。

```sh
npm run build
node scripts/prepare-deploy.mjs
wrangler deploy --config deploy/wrangler.json
```

需要已登录且具备该域名 Workers/Routes 权限的 Wrangler。`site/` 是可重建目录，不纳入 Git。发布目录只有 `/crewdemo/` 下的运行资源，移除 source map；源码和打包证据不上传。

```sh
TOUR_PUBLISH_URL=https://pendingname.com/crewdemo TOUR_PUBLISH_LABEL=live node verify-published.cjs
```

发布验收验证：路径规范化、HTML 与本地构建一致、JS/CSS 资源加载、顶部三处链接、当前进展、未来四个讨论群、协作空间四个问题、知识主题与数据交互、手机宽度。原首页需另外对比发布前后的正文 SHA256。

兼容日期 `2026-08-27` 与本次本机 Workerd 支持范围一致。此项目只部署静态资产，未添加服务端数据写入或真实 Agent 会话接入。
