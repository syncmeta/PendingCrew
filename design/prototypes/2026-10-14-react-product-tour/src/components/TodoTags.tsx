import { useState } from "react";
import { useTour } from "../state/TourContext";
import { tagsOf, todoTags } from "../domain/todoModel";
import type { Classified } from "../domain/types";
import { Button, Icon } from "./primitives";
export function TodoTags({item, edit = false}: {item: Classified; edit?: boolean}) {
  const {dispatch, setUi} = useTour();
  const tags = tagsOf(item);
  const [draft,setDraft] = useState("");
  return <div className="todo-tags">{tags.map((tag)=><Button key={tag} className="tag-chip" onClick={()=>setUi({tab:"todo",kind:"question" in item?"asks":"requests",todoTag:tag})}><Icon name="tag"/>{tag}</Button>)}
    {edit && <details className="tag-picker"><summary aria-label="编辑 待办 标签"><Icon name="tag"/> 标签</summary><div>{[...new Set([...todoTags,...tags])].map((tag)=><label key={tag}><input type="checkbox" checked={tags.includes(tag)} onChange={()=>dispatch({type:"tagTodo",id:item.id,tag})}/>{tag}</label>)}<form onSubmit={(event)=>{event.preventDefault();if(draft.trim()&&!tags.includes(draft.trim())) dispatch({type:"tagTodo",id:item.id,tag:draft});setDraft("");}}><input aria-label="新标签名称" placeholder="新标签" value={draft} onChange={(event)=>setDraft(event.target.value)}/><button type="submit">添加</button></form></div></details>}
  </div>;
}
export function TodoRail() {
  const {data:{db,ui},setUi} = useTour();
  const items = ui.kind === "asks" ? db.asks : db.requests;
  const tags = [...new Set([...todoTags,...items.flatMap(tagsOf)])];
  return <aside className="folder-rail tag-rail"><div className="folder-head"><Icon name="tag"/> 标签</div>
    {["all",...tags].map((tag)=><Button key={tag} className={"folder-button "+((ui.todoTag??"all")===tag?"selected":"")} data-todo-tag={tag} onClick={()=>setUi({todoTag:tag})}><Icon name="tag"/><span>{tag==="all"?"所有 待办":tag}</span><small>{tag==="all"?items.length:items.filter((item)=>tagsOf(item).includes(tag)).length}</small></Button>)}
  </aside>;
}
