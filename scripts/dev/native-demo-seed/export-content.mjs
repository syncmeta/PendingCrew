// Run with the existing tour's tsx loader; never reads browser/localStorage state.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {fileURLToPath, pathToFileURL} from 'node:url';
const here=path.dirname(fileURLToPath(import.meta.url));
const repo=path.resolve(here,'../../..');
const tour=path.join(repo,'design/prototypes/2026-10-14-react-product-tour');
const {initialState}=await import(pathToFileURL(path.join(tour,'src/domain/seed.ts')));
const {applyPresentationUpdates}=await import(pathToFileURL(path.join(tour,'src/domain/presentationUpdates.ts')));
const resolved=applyPresentationUpdates(initialState(true,'website'));
const selected={website:['website-main','website-todo','website-development-practice','website-development-lessons','website-program'],knowledge:['knowledge-main','knowledge-slicing','knowledge-database','knowledge-memory','knowledge-views']};
const crews=Object.entries(selected).map(([id,ids])=>{
 const record=resolved.cache[id],scene=resolved.scenes.find(s=>s.id===id);
 return {id,title:scene.crew,sessions:record.sessions,channels:ids.map(channelId=>{
  const conversation=record.db.conversations.find(c=>c.id===channelId);
  if(!conversation) throw Error('missing '+channelId);
  const group=channelId===id+'-main'?record.db:record.groups[channelId];
  const prefix=channelId.endsWith('practice')?'920':channelId.endsWith('lessons')?'930':null;
  const take=(items,kind)=>prefix?items.filter(x=>x.id.startsWith(kind+prefix)):items.slice(0,channelId.endsWith('program')?8:4);
  return {...conversation,messages:(record.db.messages[channelId]??[]).slice(0,10),requests:take(group.requests,'N'),asks:group.asks.slice(0,3),plans:take(group.plans,'W'),docs:group.docs.slice(0,4),history:group.history};
 })};
});
const sources={};
function scan(dir){for(const name of fs.readdirSync(dir).sort()){const p=path.join(dir,name);if(fs.statSync(p).isDirectory())scan(p);else if(/\.(ts|json)$/.test(name)&&!name.includes('.test.'))sources[path.relative(repo,p)]=crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');}}
scan(path.join(tour,'src/domain'));
for(const name of ['content.json','fixtures.json','topicCrews.json','documentLibrary.json']){const p=path.join(tour,'src',name);sources[path.relative(repo,p)]=crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');}
const out={kind:'pendingcrew.native-html-demo.v1',sourcePipeline:'initialState(true,website) -> applyPresentationUpdates; workspaceRefresh -> developmentExamples -> nativeDemo -> contentCompletion + topic/document/presentation refresh',notice:'演示资料：来自现有 HTML Demo 的静态快照。成员均停止，计划仅展示，不代表真实执行或当前审批。',sources,crews};
fs.writeFileSync(path.join(here,'content.json'),JSON.stringify(out,null,2)+'\n');
console.log(JSON.stringify(crews.map(c=>({crew:c.title,roles:c.sessions.length,channels:c.channels.map(ch=>({title:ch.title,messages:ch.messages.length,aiTodos:ch.requests.length,humanTodos:ch.asks.length,plans:ch.plans.length,docs:ch.docs.length}))})),null,2));
