import {createElement} from "react";
import {renderToStaticMarkup} from "react-dom/server";
import {DocumentContent} from "../components/DocumentContent";
import {test} from "node:test";
import assert from "node:assert/strict";
import {initialState} from "./seed";
import {reducer} from "./reducer";
import {switchConversation,crewLedgers} from "./selectors";
import {applyNativeDemo,programWork} from "./nativeDemo";
import site from "./siteDocuments.json";
import snapshot from "./programSnapshot.json";

test("crew totals contain all sessions, child crew membership is reconciled",()=>{
 const state=initialState(),intro=state.cache.website;
 assert(intro.sessions!.length>=8);
 const main=intro.db.conversations.find((c)=>c.id==="website-main")!;
 for(const session of intro.sessions!) assert(main.members!.some((m)=>m.name===session.name));
 const trip=state.cache.trip.db.conversations.find((c)=>c.id==="trip-main")!;
 for(const session of state.cache.research.sessions!) assert(trip.members!.some((m)=>m.name===session.name));
 assert.strictEqual(applyNativeDemo(state),state);
});
test("site documents retain original bodies and images stay outside persisted state",()=>{
 const state=initialState();let found=0;
 for(const record of Object.values(state.cache))for(const [,db]of crewLedgers(record))for(const doc of db.docs.filter((d)=>d.siteSource)) {
  const original=site.find((s)=>s.siteSource===doc.siteSource)!;
  assert.equal(doc.body,original.body);assert.equal(doc.sourceSha256,original.sourceSha256);assert.equal(doc.siteAssets,undefined);found++;
 }
 assert(found>20);assert(JSON.stringify(state).length<1_000_000);
});
test("creating human Todo does not create AI Todo; navigation changes do not alter work",()=>{
 let state=initialState();switchConversation(state.cache.website,"website-development-practice");
 const count=state.cache.website.db.requests.length, human=state.cache.website.db.asks.length;
 state=reducer(state,{type:"newHumanTodo",text:"确认下一轮真人验收场景"});
 assert.equal(state.cache.website.db.requests.length,count);assert.equal(state.cache.website.db.asks.length,human+1);
 assert.equal(state.cache.website.ui.kind,"asks");
 const ask=state.cache.website.db.asks[0];state=reducer(state,{type:"answer",id:ask.id,text:ask.options[0]});assert.equal(state.cache.website.db.asks[0].status,"answered");
 const db=structuredClone(state.cache.website.db);
 state=reducer(state,{type:"toggleCrew",id:"website"});state=reducer(state,{type:"toggleSidebar"});assert.deepEqual(state.cache.website.db,db);assert(state.navigation!.sidebarCollapsed);
});
test("program plan and progress use one editable canonical work graph",()=>{
 let state=initialState();const work=programWork(state)!;
 assert.equal(work.plans.length,snapshot.plans.length);assert(work.plans.some((p)=>p.parent&&work.plans.some((parent)=>parent.id===p.parent&&parent.parent)));
 switchConversation(state.cache.website,"website-program");
 state=reducer(state,{type:"savePlan",id:"W94043",status:"active",owner:"远程与配对",note:"先验证持久配对后的重新连接。"});
 switchConversation(state.cache.website,"website-main");
 assert.equal(programWork(state)!.plans.find((p)=>p.id==="W94043")?.note,"先验证持久配对后的重新连接。");
});

test("original document embeds render offline images instead of wiki syntax",()=>{
 const source=site[0];
 const markup=renderToStaticMarkup(createElement(DocumentContent,{body:source.body,assets:source.assets}));
 assert(markup.includes("<figure>"));assert(markup.includes("data:image/png;base64,"));
 assert(markup.includes("PendingCrew 主界面"));assert(!markup.includes("![["));
 assert(markup.includes('href="https://github.com/syncmeta/PendingCrew"'));
});

test("archived crews without sessions do not trigger repeated interface migrations",()=>{
 const state=initialState();
 state.cache["self-introduction"] && delete state.cache["self-introduction"].sessions;
 assert.strictEqual(applyNativeDemo(state),state);
});

test("new crew reset cannot collide with site-document identities from archived crews",()=>{
 let state=initialState();
 state=reducer(state,{type:"newCrew",id:"custom-native-identity",name:"新开发分组",brief:"整理一份开发记录"});
 state=reducer(state,{type:"reset"});
 const ids=Object.values(state.cache).flatMap((record)=>crewLedgers(record).flatMap(([,db])=>db.docs.map((doc)=>doc.id)));
 assert.equal(new Set(ids).size,ids.length);
});

test("new AI and human Todo identities remain numeric and unique beside semantic legacy IDs",()=>{
 let state=initialState();switchConversation(state.cache.website,"website-development-practice");
 for(let i=0;i<2;i++)state=reducer(state,{type:"newRequest",text:"AI 新任务 "+i});
 for(let i=0;i<2;i++)state=reducer(state,{type:"newHumanTodo",text:"真人新事项 "+i});
 const ids=[...state.cache.website.db.requests.slice(0,2),...state.cache.website.db.asks.slice(0,2)].map((item)=>item.id);
 assert(ids.every((id)=>Number.isFinite(Number(id.slice(1)))));assert.equal(new Set(ids).size,4);
});
