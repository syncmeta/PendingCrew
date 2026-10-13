# M2 documents and independent view folders — agreed implementation contract

This freezes client/backend DTO shape for parallel implementation, not completed functionality. Backend must implement formal schema10 migration/backup, trusted ACL, transactions and actual acceptance before advertising `documents` / `viewFolders` hello capabilities. Old helpers with no capability remain disabled. No Jev/memory implementation or automatic model wake.

## Documents

RPC names `document.create`, `document.get`, `document.list`, `document.update`, `document.delete`. MCP snake names `document_create/get/list/update/delete`; parameters use snake_case there, camelCase on RPC.

- create `{requestId: UUID, crewId, title, body}` -> `{document: DocumentSummary, requestId, reused: bool}`. Server document ID opaque UUID. Normalize request UUID; exact immutable original crew/title/body witness, not current edited fields, determines retry. Same original request returns current metadata, never body or repeated events. Changed original payload conflicts. Body is exact immutable-create payload (updates are separate CAS).
- get `{id}` -> `DocumentDetail` (summary fields plus `body`). Deleted content retained for audit and detail marked deleted; normal lists exclude it unless `includeDeleted:true`.
- list `{crewId, after?: UUID, limit?: u32, includeDeleted?: bool}` -> `{items:[DocumentSummary], next?: UUID}`; limit default50/max100, stable UUID order, no body.
- update `{id,expectedRevision:string,title,body}` -> `DocumentSummary`.
- delete `{id,expectedRevision:string}` -> `DocumentSummary`; soft deletion, no history/body destruction or wake.
- Summary `{id,crewId,title,revision:string,bodyLength:u64,createdBy: Actor,updatedBy:Actor,createdAt:string,updatedAt:string,deletedAt?:string}`. bodyLength UTF-8 bytes. Actor `{kind:"human"|"session",sessionId?:string}`; timestamps existing RFC3339 UTC; decimal revisions canonical strings.
- update/delete are actual current CAS with finite conflict facts current summary, no repeated body. One crew real members collaborate; creator provenance does not lock editing. Frozen crews read-only. Human local trust stays existing; attached agent ordinary RPC reads/writes reject, trusted MCP actor checks scope.

## Four independent view folder scopes

RPC names `viewFolder.list/create/update/delete/assign`. MCP names `view_folder_list/create/update/delete/assign`. Scope always `{crewId,view}`; view enum `messages|todo|documents|progress`. These classifications never alter plan parent/status/owner. One object has at most one folder in each container crew/view.

- list `{crewId,view}` -> `FolderSnapshot {crewId,view,treeRevision:string,folders:[Folder],assignments:[FolderAssignment]}`. No document body/message content/work goal embedded. Live visibility applied to classification refs and counts; no private channel title inference.
- create `{crewId,view,requestId:UUID,expectedTreeRevision:string,name,parentId?:UUID}` -> `FolderSnapshot`. Exact original create witness; repeated successful same request returns latest visible snapshot without changing tree revision/events. Changed payload conflicts.
- update `{crewId,view,id,expectedTreeRevision:string,name,parentId?:UUID,position:u32}` -> `FolderSnapshot`. Full replacement name/parent/position; null/omitted parent means root. CAS covers whole scope, cycle guard, folder depth independent of work graph max3; implementation limit32 prevents unbounded recursion.
- delete `{crewId,view,id,expectedTreeRevision:string}` -> `FolderSnapshot`. Children promoted to deleted folder parent maintaining order; direct members become uncategorized; no underlying object/history deleted.
- assign `{crewId,view,expectedTreeRevision:string,object:ObjectRef,folderId?:UUID}` -> `FolderSnapshot`. null/omitted folder unassigns. Single CAS transaction; unchanged assignment no event/revision increment. New folder insertion appended; update explicit position reorders siblings.
- Folder `{id,name,parentId?:UUID,position:u32}`; Assignment `{object:ObjectRef,folderId:UUID}`. Snapshot revision decimal canonical. Folder counts derived from visible assignments client-side, not authoritative work lifecycle.
- ObjectRef tagged `{kind:"channel"|"requirement"|"work"|"humanTask"|"document",id:string}`. Channel stable channel ID, requirement global `#`, work global `W`, humanTask global `H`, doc UUID. messages accepts channel; todo accepts requirement/work/humanTask; progress accepts work; documents accepts document. Source/H/W classification object crew must match container scope. Selected crew never captures foreign H from organization inbox.
- Sole crosscrew exception: real currently visible DM can be classified under actor's container crew/messages, without moving channel crew. Every mutation/list/detail checks live actor visibility, including after membership changes. Foreign invisible channel always rejected. Attached agents use trusted MCP for reads as well as writes, cannot default to Human RPC.
- Freeze read-only. Crew purge removes its own documents/folders/classifications in same authority transaction; independent crosscrew channel itself survives. Formal history preserves actual actors, CAS and exact doc content edits; no second work ledger.

## Acceptance and delivery

Before enabling capabilities: real old-v9 upgrade/backup/reopen/failure rollback; idempotent create after edited doc; CAS concurrency and frozen scopes; authenticated Human/member/foreign boundaries including attached direct read/write bypass; private DM visibility change list/count/reference guards; all folder operations across each view with unchanged work parents/state; actual daemon + typed Swift interactive document edits and independent filters. A contract-only page is not counted as delivered UI or backend.

## Frozen bounds and refresh facts

Document title <=512 UTF-8 bytes; body <=1 MiB. Folder name nonblank trimmed <=256 UTF-8 bytes; each crew/view scope <=256 folders and <=4096 assignments, so full snapshot is bounded. RPC CAS returns existing CONFLICT (-32009), data.currentDocument:DocumentSummary or data.currentTreeRevision:string; MCP refuses with the same finite facts.

`document.changed {crewId,id,revision,deleted:bool}` uses subscribe topic `documents`; `viewFolder.changed {crewId,view,treeRevision}` uses topic `viewFolders`. Subscribe only when corresponding capability is present. No visible assignments still returns the current scope treeRevision for legitimate CAS; this revision is not an object count. All refs/counts remain live-filtered.

Folder names are intentionally shared, manually authored crew metadata; empty folders remain visible. They do not inherit DM privacy. The system never derives/copies hidden channel titles or participant identities into metadata, and returns no hidden-item count. Purging a crew must reject live structured foreign folder/channel references with finite stable conflict refs for explicit Human unassignment; invisibility never silently deletes a classification. No arbitrary document-body text scanning.

## Agent output budget

Ordinary Human RPC folder mutations return the bounded full snapshot for UI reconciliation. Authenticated MCP mutations instead return only `{crewId,view,treeRevision,folderId?}` (assignment adds the changed object ref and nullable folderId); deletion does not reinject the whole promoted subtree. Agent `view_folder_list` adds `part:folders|assignments` (default folders), `after?:string`, `limit?:u32` (default50/max100). It returns `{crewId,view,treeRevision,items,next?}` for that part. Folder cursor is UUID; assignment cursor is an opaque canonical encoding of kind+id. Each page independently applies live visibility. These are projections of the same authority, not a second classification ledger. No automatic complete-tree injection after a model writes one classification.
