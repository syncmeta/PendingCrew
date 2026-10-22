import examples from "./developmentExamples.json";
import type { TourState, GroupWork, Conversation, Message } from "./types";
export function applyDevelopmentExamples(state: TourState): TourState {
  const existing=state.cache.website;
  if (state.developmentRevision===1 && examples.every((item)=>existing.db.conversations.some((c)=>c.id===item.id))) return state;
  const next=structuredClone(state), record=next.cache.website;
  record.groups ??= {};
  for (const item of examples) {
    if (record.db.conversations.some((c)=>c.id===item.id)) continue;
    const conversation:Conversation={id:item.id,title:item.title,subtitle:item.subtitle,kind:"group",members:item.members};
    record.db.conversations.push(conversation);
    record.db.messages[item.id]=structuredClone(item.messages) as Message[];
    record.groups[item.id]=structuredClone(item.work) as GroupWork;
  }
  record.updatedAt=new Date().toISOString();
  next.developmentRevision=1;
  return next;
}
