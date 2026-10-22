import {test} from "node:test";
import assert from "node:assert/strict";
import {existsSync} from "node:fs";
import {resolve} from "node:path";
import {initialState} from "./seed";
import {reducer} from "./reducer";
import {crewLedgers,findReference,switchConversation} from "./selectors";
import {applyDevelopmentExamples} from "./developmentExamples";

test("real development examples link tasks, plans, evidence and sourced data",()=>{
 const state=initialState(),scene=state.cache.website;
 for (const id of ["website-development-practice","website-development-lessons"]) {
  const db=crewLedgers(scene).find(([conv])=>conv===id)![1];
  assert(db.requests.every((r)=>r.owner&&r.source));
  for(const plan of db.plans) {
   for(const source of plan.sources) assert(findReference(state,source));
   if(plan.evidence) assert(findReference(state,plan.evidence));
  }
  assert(db.docs.some((d)=>d.dataView?.kind==="table"));
  for(const doc of db.docs) for(const source of doc.sources??[]) assert(source.path.startsWith("https://docs.pendingname.com/") || existsSync(resolve("../../..",source.path)));
  for(const msg of scene.db.messages[id]) if(msg.ref) assert(findReference(state,msg.ref));
 }
 assert.strictEqual(applyDevelopmentExamples(state),state);
});

test("Todo assignment and development migration preserve visitor work and active conversation",()=>{
 let state=initialState();switchConversation(state.cache.website,"website-development-practice");
 state=reducer(state,{type:"assignTodo",id:"N92003",owner:"Todo入口改造"});
 assert.equal(state.cache.website.db.requests.find((r)=>r.id==="N92003")?.owner,"Todo入口改造");
 state.cache.website.ui.drafts["website-development-practice"]="我未发送的草稿";
 state.cache.website.db.docs[0].body+="\n我的经验";
 delete state.developmentRevision;
 const next=applyDevelopmentExamples(state);
 assert.equal(next.cache.website.ui.conv,"website-development-practice");
 assert.equal(next.cache.website.db.requests.find((r)=>r.id==="N92003")?.owner,"Todo入口改造");
 assert(next.cache.website.db.docs[0].body.endsWith("我的经验"));
 assert.equal(next.cache.website.ui.drafts["website-development-practice"],"我未发送的草稿");
 assert.equal(next.cache.website.db.conversations.filter((c)=>c.id==="website-development-practice").length,1);
});
