import history from "./captainHistory.json";
import type { TourState, SceneState, Ledger, Scene } from "./types";
import { switchConversation, crewLedgers } from "./selectors";
export function applyWorkspaceRefresh(state: TourState): TourState {
  if (state.workspaceRevision === 1 && state.cache.captain && !state.cache.website.db.conversations.some((c) => ["website-agent", "website-planning", "website-development"].includes(c.id))) return state;
  const next = structuredClone(state);
  const website = next.cache.website;
  const removed = new Set(["website-agent", "website-planning", "website-development"]);
  if (removed.has(website.ui.conv)) switchConversation(website, "website-main");
  website.db.conversations = website.db.conversations.filter((c) => !removed.has(c.id));
  // Preserve archived group records and visitor messages; they no longer appear as conversations.
  if (!next.cache.captain) {
    const requests = history.map((item, i) => ({id: "N" + (90001+i), code: "#" + (i+1), text: item.text,
      source: item.source, tags: i < 2 ? ["前端", "小工程"] : i % 3 === 0 ? ["紧急", "后端", "大工程"] : i % 2 ? ["前端", "小工程"] : ["后端", "大工程"]}));
    const plans: Ledger["plans"] = requests.map((r, i) => ({id: "W"+(90001+i), code: "P"+(i+1), title: r.text,
      parent: null, status: i === 3 ? "active" : "open", owner: "机长", sources: [r.id], dependsOn: [], blockedOn: [],
      note: i === 3 ? "先确定折叠规则，再完成渲染层和回归验证。" : "已收下，按上下文和紧急程度安排。", nextCheck: "下一轮检查", tags: r.tags}));
    plans.push({id:"W90100",code:"P4.1",title:"实现折叠控件与展开行为",parent:"W90004",status:"open",owner:"机长",sources:[requests[2].id],dependsOn:[],blockedOn:[],note:"默认保留判断和结论，长内容可展开。",nextCheck:"界面检查"});
    const db: Ledger = {requests, plans, asks:[{
      id:"H90001",question:"群聊消息默认折叠到什么程度？",recommendation:"只折代码块、表格和终端输出",options:["只折代码块、表格和终端输出", "也折叠长引用和列表"],status:"open",answer:"",item:"W90004",context:"摘自历史人类 Todo #1；此处演示 Agent 请真人判断的流程。",tags:["前端", "紧急"]
    },{id:"H90002",question:"消息超过几行时折叠？",recommendation:"超过 8 行",options:["超过 8 行", "超过 4 行", "超过 15 行"],status:"open",answer:"",item:"W90004",context:"摘自历史人类 Todo #2；这里的待答状态仅用于演示。",tags:["前端", "小工程"]}],
      docs:[{id:"D90001",title:"这些零碎任务从哪里来",author:"机长",kind:"工程说明",body:"这些条目摘自 PendingCrew 真实历史记录。任务文字来自 Agent Todo 与计划快照，待办状态和排列为演示重建，不代表项目当前仍未完成。\n\n输入框、消息折叠、投递重放、引用跳转、翻历史时的位置保持，往往是陆续出现的零碎需求。你可以继续添加 Todo；机长按标签组织它们，并把相关工作关联到计划/进展。",sources:[{label:"历史 Agent Todo",path:"docs/internal/2026-09-13-ledger-snapshot/todos.json"},{label:"历史计划记录",path:"docs/internal/2026-09-13-ledger-snapshot/plan.json"},{label:"历史人类 Todo",path:"docs/internal/2026-09-13-ledger-snapshot/human-todos.json"}]}],
      conversations:[{id:"captain-main",title:"任务流",kind:"group",subtitle:"源源不断的零碎任务，交给机长持续跟进。",members:[{name:"机长",detail:"Codex Session",glyph:"🦉"},{name:"人",detail:"人类",glyph:"👤"}]}],
      messages:{"captain-main":[{id:"captain-context",sender:"机长",text:"这里摘选了 PendingCrew 历史里的真实任务，演示怎样持续收下零碎需求。任务状态为模拟，可在右侧 Todo 和计划/进展中交互。"},...requests.slice().reverse().map((r,i)=>({id:"captain-history-"+i,sender:"你",text:r.text,ref:r.id})),{id:"captain-reply",sender:"机长",text:"都收下了。同一条 Todo 可以同时标记紧急、前端或后端；需要你判断的事情单独放在给真人一栏。",ref:"H90001"}]},
      folders:{message:["任务沟通"],todo:[],document:["历史记录"],progress:["工作计划"]},placements:{},history:[]};
    const scene: Scene = {...structuredClone(next.scenes.find((s)=>s.id==="website")!),id:"captain",name:"机长",crew:"机长",agent:"机长",project:"持续跟进零碎任务",initialLedger:structuredClone(db),initialGroups:{}};
    const record: SceneState = {db,groups:{},updatedAt:new Date().toISOString(),ui:{...structuredClone(website.ui),conv:"captain-main",tab:"message",query:"",doc:null,collapsed:[],drafts:{},scrolls:{},todoTag:"all",selectedFolders:{message:"all",todo:"all",document:"all",progress:"all"}}};
    next.scenes.push(scene);next.cache.captain=record;
  }
  for (const scene of Object.values(next.cache)) {
    scene.ui.todoTag ??= "all";
    for (const [,db] of crewLedgers(scene)) {
      for (const item of [...db.requests,...db.asks]) item.tags ??= [];
    }
  }
  next.workspaceRevision=1;
  return next;
}
