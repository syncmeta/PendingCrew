# 群聊搜索契约

PendingCrew 的当前群、跨群和 session MCP 搜索统一调用
`CrewMessageSearch`。PendingBot 不直接编译依赖此 Swift 文件，但需保持下列 wire 语义。

## 查询语义

- `query` 按 Unicode 空白拆词；每个词都必须命中，词可分布在不同字段（AND）。
- 每个字段先做大小写、音标和全/半角折叠，再做 substring；中文不另行分词。
- 可搜索字段只有正文、发送者显示名/稳定 id、附件 filename/MIME、ISO8601 时间文本。
- crew/conversation 标题只用于展示和定位，不参与匹配；附件路径、文件内容不参与匹配。
- `after`、`before` 都是可选 ISO8601，边界包含；无效时间报错，`after > before` 报错。
- 默认最新优先；默认 50 条；`limit` 收口到 1...200。

## 结果语义

PendingCrew 结果字段：

```text
crewId, crewTitle, messageId, text,
senderName, senderId?, createdAt,
attachmentMetadata[], matchedFields[]
```

`matchedFields` 只能是 `text | sender | attachment | time`。PendingBot 映射
`crewId/crewTitle` 为 `conversationId/conversationTitle`，并额外返回
`conversationType`、`messageSeq`；客户端用 `conversationId + messageSeq` 调
around-message 读取契约后定位旧消息，不能依赖最近 200 条本地缓存。

## session MCP

- `search_whiteboard(query, after?, before?, limit?)` 搜索当前 crew 的完整白板，返回
  `crew_id`、`message_id`、时间、发送者、命中字段和正文，结果最新优先。
- `read_whiteboard(limit?, before?)` 只做分页浏览：默认最近 50 条，最多 200 条；
  `before` 是消息 id 游标且不重复游标消息。它不再承担搜索时的全量历史返回。
