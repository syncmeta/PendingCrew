import { productText } from "../domain/displayText";
import { useTour } from "../state/TourContext";
import type { HumanAsk, Request } from "../domain/types";
export function TodoOwner({item,human=false}:{item:Request|HumanAsk;human?:boolean}) {
  const {data:{db,ui},scene,dispatch}=useTour();
  const owner=item.owner??(human?"你":db.plans.find((p)=>p.sources.includes(item.id))?.owner??scene.agent);
  const members=db.conversations.find((c)=>c.id===ui.conv)?.members?.map((m)=>m.name)??[scene.agent,"你"];
  return <label className="todo-owner">负责人<select aria-label={"待办 "+item.id+" 负责人"} value={owner} onChange={(event)=>dispatch({type:"assignTodo",id:item.id,owner:event.target.value})}>{[...new Set([owner,...members,"你"])].map((name)=><option key={name} value={name}>{productText(name)}</option>)}</select></label>;
}
