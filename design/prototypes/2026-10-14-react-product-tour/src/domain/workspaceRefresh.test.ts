import {test} from "node:test";
import assert from "node:assert/strict";
import {initialState} from "./seed";
import {reducer} from "./reducer";
import {applyWorkspaceRefresh} from "./workspaceRefresh";
import {todoCounts} from "./todoModel";
import {switchConversation} from "./selectors";

test("captain contains sourced historical tasks, two-way Todo and one plan model",()=>{
  const state=initialState();switchConversation(state.cache.website,"website-development-practice");const record=state.cache.website;
  assert(record.db.requests.length>=10);
  assert(record.db.requests.filter((r)=>r.id.startsWith("N900")).every((r)=>r.source?.startsWith("docs/internal/") && r.tags!.length>1));
  assert.equal(record.db.asks.length,2);
  assert(todoCounts(record).red>=11);assert(todoCounts(record).yellow>=2);
  assert(record.db.plans.some((p)=>p.parent));
  const docs=record.db.docs.find((doc)=>doc.id==="D90001")!;assert(docs.body.includes("不代表项目当前仍未完成"));
  assert.strictEqual(applyWorkspaceRefresh(state),state);
});

test("labels are many-to-many and completion changes crew counts independently of tags",()=>{
  let state=initialState();switchConversation(state.cache.website,"website-development-practice");
  const initialCounts=todoCounts(state.cache.website);
  const id="N90001";
  state=reducer(state,{type:"tagTodo",id,tag:"紧急"});
  const tagged=state.cache.website.db.requests.find((r)=>r.id===id)!;
  assert.deepEqual(tagged.tags,["前端","小工程","紧急"]);
  state=reducer(state,{type:"tagTodo",id,tag:"小工程"});
  assert.deepEqual(state.cache.website.db.requests.find((r)=>r.id===id)!.tags,["前端","紧急"]);
  state=reducer(state,{type:"completeTodo",id});
  assert.equal(todoCounts(state.cache.website).red,initialCounts.red-1);
  assert.equal(todoCounts(state.cache.website).yellow,initialCounts.yellow);
  assert.deepEqual(state.cache.website.db.requests.find((r)=>r.id===id)!.tags,["前端","紧急"]);
  const ask=state.cache.website.db.asks[0];
  state=reducer(state,{type:"answer",id:ask.id,text:ask.options[0]});
  assert.equal(todoCounts(state.cache.website).yellow,initialCounts.yellow-1);
});

test("removing legacy conversations preserves archived work and resets cannot resurrect them",()=>{
  const before=initialState();delete before.workspaceRevision;
  const website=before.cache.website;
  website.db.conversations.push({id:"website-agent",title:"机长",kind:"dm",subtitle:"旧会话"});
  switchConversation(website,"website-agent");
  website.db.requests.push({id:"N990001",code:"#访客",text:"访客原有任务"});
  website.ui.drafts["website-agent"]="旧会话草稿";
  const next=applyWorkspaceRefresh(before);
  assert.equal(next.cache.website.ui.conv,"website-main");
  assert(next.cache.website.groups!["website-agent"].requests.some((r)=>r.id==="N990001"));
  assert.equal(next.cache.website.ui.drafts["website-agent"],"旧会话草稿");
  next.sceneId="website";
  const reset=reducer(next,{type:"reset"});
  assert.deepEqual(reset.cache.website.db.conversations.filter((c)=>c.kind==="group").map((c)=>c.title),["总群","Todo介绍","实际开发演示","经验与复盘","大工程"]);
  assert.equal(reset.scenes.filter((s)=>s.id==="captain").length,0);
});
