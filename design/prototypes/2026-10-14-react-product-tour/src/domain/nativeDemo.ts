import site from "./siteDocuments.json";
import snapshot from "./programSnapshot.json";
import type { TourState, GroupWork, Ledger, DocumentRecord, Plan } from "./types";
import {crewLedgers} from "./selectors";
const development = [
 ["captain","机长","协调开发与交付","🦉"],
 ["todo","Todo介绍","人机两套 Todo","🦊"],
 ["composer","Todo入口改造","输入体验","🛠️"],
 ["core","Core/持久化","统一工作账与恢复","🗃️"],
 ["native","原生客户端","Mac / iOS 界面","🖥️"],
 ["remote","远程与配对","iOS ↔ VPS 持久配对","🔗"],
 ["acp","跨厂商执行","PTY / ACP 生命周期","🔌"],
 ["qa","独立质量","完整用户流程验收","🔎"],
 ["search","搜索核验","搜索语义与证据","📚"],
 ["followup","跟进与调度","事实驱动跟进","⏱️"],
];
const emptyWork=():GroupWork=>({plans:[],requests:[],asks:[],docs:[],folders:{message:[],todo:[],document:["文档站"],progress:["大工程"]},placements:{},history:[]});
export function applyNativeDemo(state:TourState):TourState {
 const website=state.cache.website;
 if(state.interfaceRevision===1 && website.db.conversations.some((c)=>c.id==="website-program") && state.scenes.every((scene)=>state.cache[scene.id]?.sessions?.length)) return state;
 const next=structuredClone(state);
 const usedDocIds=new Set(Object.values(next.cache).flatMap((record)=>crewLedgers(record).flatMap(([,db])=>db.docs.map((doc)=>doc.id))));
 next.navigation??={collapsedCrews:next.scenes.filter((s)=>s.id!=="website").map((s)=>s.id),sidebarCollapsed:false};
 const intro=next.cache.website;intro.groups??={};
 if(!intro.db.conversations.some((c)=>c.id==="website-program")) {
  intro.db.conversations.push({id:"website-program",title:"大工程",subtitle:"2026-10-22 计划与进展快照",kind:"group"});
  intro.groups["website-program"]={...emptyWork(),plans:structuredClone(snapshot.plans) as Plan[],requests:[{id:"N94001",code:"#大工程",text:"把本机协作、跨厂商执行和远程多端交付推进成完整的人机协作系统。",owner:"机长",tags:["大工程"],status:"active",source:snapshot.source,sourceLabel:"大工程快照 · "+snapshot.asOf}],history:[{text:"按总计划与验收记录整理，状态截于 "+snapshot.asOf,time:snapshot.asOf}]};
  intro.groups["website-program"].asks.push({id:"H94001",question:"下一轮 iOS 远程配对真人验收，优先检查哪个场景？",owner:"人",context:"根据 2026-10-22 大工程验收范围改编的演示事项，不是当前真实审批记录。",recommendation:"先检查重开后手动连接，再核业务读写和撤权。",options:["重开后手动连接","业务读写与撤权"],status:"open",answer:"",item:"W94043",tags:["大工程"]});
  intro.db.messages["website-program"]=[{id:"program-snapshot-main",sender:"机长",text:"这是正在推进的大工程。计划/进展按 2026-10-22 总计划整理：本机工作闭环、Mac 交付和 Linux 受管恢复已有对应证据；当前推进 iOS ↔ VPS 持久配对。",ref:"W94001"},{id:"program-snapshot-next",sender:"远程与配对",text:"邀请文件和长码表单的实际流程已经验证；三阶段交换、持久保存、重开手动连接、业务读写和撤权仍要按完整链路推进。",ref:"W94040"},{id:"program-snapshot-acp",sender:"跨厂商执行",text:"基础组件证据继续复用。正式 ACP 合同、完整历史和生命周期尚未闭合，不能把局部组件通过算作全部完成。",ref:"W94050"}];
 }
 for(const [index,scene] of next.scenes.entries()) {
  const record=next.cache[scene.id];record.groups??={};
  if(!record.sessions?.length) {
   const roles=scene.id==="website"?development:[["lead",scene.agent,scene.project,scene.glyph]];
   record.sessions=roles.map(([key,name,role,glyph],i)=>({id:scene.id+"-session-"+key,name,detail:(i===2?"Claude Code":"Codex")+" Session · "+role,glyph}));
  }
  for(const session of record.sessions) {
   if(record.db.conversations.some((c)=>c.id===session.id)) continue;
   record.db.conversations.push({id:session.id,title:session.name,kind:"dm",subtitle:session.detail,members:[session,{name:"人",detail:"人类",glyph:"👤"}]});
   record.groups[session.id]=emptyWork();
   const plan=scene.id==="website"?snapshot.plans.find((p)=>p.owner===session.name):undefined;
   record.db.messages[session.id]=[{id:session.id+"-intro",sender:session.name,text:scene.id==="website"?`我是负责${session.detail.split(" · ")[1]}的 Session。这里展示 PendingCrew 开发中的分工；同一机组的总群汇总全部 Session，具体工作仍有自己的群和计划。`:`这个 Session 负责${scene.crew}。群内文档直接来自文档站。`},...(plan?[{id:session.id+"-work",sender:session.name,text:plan.note,ref:plan.id}]:[])];
  }
  const local=record.sessions;
  const descendants=scene.id==="trip"?(next.cache.research.sessions??[]):[];
  const members=[...new Map([...local,...descendants].map((m)=>[m.name,{name:m.name,detail:m.detail,glyph:m.glyph}])).values(),{name:"人",detail:"人类",glyph:"👤"}];
  const main=record.db.conversations.find((c)=>c.id===scene.id+"-main");if(main) main.members=members;
  if(scene.id==="website") {
   const program=record.db.conversations.find((c)=>c.id==="website-program");if(program) program.members=members;
   const practice=record.db.conversations.find((c)=>c.id==="website-development-practice");if(practice)practice.members=members.filter((m)=>["机长","Todo入口改造","原生客户端","Core/持久化","独立质量","人"].includes(m.name));
   const lessons=record.db.conversations.find((c)=>c.id==="website-development-lessons");if(lessons)lessons.members=members.filter((m)=>["机长","搜索核验","独立质量","人"].includes(m.name));
  }
  const visible=new Set(record.db.conversations.map((c)=>c.id));
  for(const [conv,db] of crewLedgers(record)) {
   if(!visible.has(conv))continue;
   for(const doc of db.docs)if(!doc.dataView && !doc.siteSource && doc.author!=="你")doc.archived=true;
   if(db.docs.some((doc)=>doc.siteSource))continue;
   let picks=scene.id==="website"?[0,4]:scene.id==="knowledge"?[10]:scene.id==="captain"?[1,2]:[4,3];
   if(conv==="website-todo")picks=[5,6];
   else if(conv==="website-development-practice")picks=[2,1];
   else if(conv==="website-development-lessons")picks=[8,7];
   else if(conv==="website-program")picks=[9,2];
   else if(conv.includes("-session-"))picks=[scene.id==="website"?1:4];
   for(const [n,pick] of picks.entries()) {
    const doc=site[pick];
    let number=980000+index*1000+record.db.conversations.findIndex((c)=>c.id===conv)*20+n;
    while(usedDocIds.has("D"+number))number++;
    const id="D"+number;usedDocIds.add(id);
    db.docs.push({id,title:doc.title,body:doc.body,siteSource:doc.siteSource,sourceSha256:doc.sourceSha256,sources:doc.sources,author:"PendingCrew 文档站",folder:"文档站",kind:"产品说明"} as DocumentRecord);
   }
   if(!db.folders.document.includes("文档站"))db.folders.document.unshift("文档站");
  }
 }
 // Research may have been initialized after its parent; reconcile parent main membership once.
 const trip=next.cache.trip, research=next.cache.research;
 const tripMain=trip.db.conversations.find((c)=>c.id==="trip-main");
 if(tripMain)tripMain.members=[...new Map([...(trip.sessions??[]),...(research.sessions??[])].map((m)=>[m.name,{name:m.name,detail:m.detail,glyph:m.glyph}])).values(),{name:"人",detail:"人类",glyph:"👤"}];
 next.interfaceRevision=1;
 return next;
}
export function programWork(state:TourState):Ledger|undefined {
 const record=state.cache.website;
 return record.ui.conv==="website-program"?record.db:record.groups?.["website-program"]?{...record.db,...record.groups["website-program"]}:undefined;
}
