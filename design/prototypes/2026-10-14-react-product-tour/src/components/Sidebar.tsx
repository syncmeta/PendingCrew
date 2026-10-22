import { useTour } from "../state/TourContext";
import type { Scene } from "../domain/types";
import { Button, Icon } from "./primitives";
import { todoCounts } from "../domain/todoModel";
import { QuotaFooter } from "./QuotaFooter";
import { ResizeHandle } from "./ResizeHandle";
import { formatUpdateTime, initialContentUpdatedAt } from "../domain/presentationUpdates";
function Disclosure({open,onClick,label}:{open:boolean;onClick:()=>void;label:string}) {
  return <Button action="disclosure" className={"disclosure "+(open?"open":"")} aria-label={(open?"折叠":"展开")+label} aria-expanded={open} onClick={onClick}><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m6 3.5 4.5 4.5L6 12.5"/></svg></Button>;
}
function CrewRow({scene,indent=0,children}:{scene:Scene;indent?:number;children?:React.ReactNode}) {
  const {state,selectScene,dispatch}=useTour();
  const record=state.cache[scene.id],counts=todoCounts(record),updatedAt=record.updatedAt??initialContentUpdatedAt,updated=formatUpdateTime(updatedAt);
  const open=!(state.navigation?.collapsedCrews??[]).includes(scene.id);
  return <>
    <div className="crew-entry" style={{paddingLeft:indent}}>
      {children && <Disclosure open={open} label={scene.crew} onClick={()=>dispatch({type:"toggleCrew",id:scene.id})}/>}
      <Button action="scene" data-id={scene.id} className={"crew-row "+(scene.id===state.sceneId?"selected":"")} onClick={()=>selectScene(scene.id)}>
        <span className="crew-text"><span className="crew-top"><span>{scene.crew}</span><small title={"最近更新："+updated.full}><time dateTime={updatedAt}>{updated.short}</time></small></span>
          <span className="crew-todo-counts"><span className="todo-count" title="AI Todo 未完成" aria-label={counts.red+" 个 AI Todo 未完成"}><i className="attention-dot red"/>{counts.red}</span><span className="todo-count" title="真人 Todo 待处理" aria-label={counts.yellow+" 个真人 Todo 待处理"}><i className="attention-dot yellow"/>{counts.yellow}</span></span>
        </span>
      </Button>
    </div>
    {open && children && <div className="crew-children">
      {children}
    </div>}
  </>;
}
export function Sidebar() {
  const {state,data:{ui},setUi,setModal,dispatch}=useTour();
  const closed=state.navigation?.collapsedCrews??[];
  const order=["website","captain","problems","knowledge","current-progress","future-five-days","trip","research","administration"];
  const scenes=state.scenes.slice().sort((a,b)=>(order.includes(a.id)?order.indexOf(a.id):order.length)-(order.includes(b.id)?order.indexOf(b.id):order.length));
  const collaborationChildren=scenes.filter((scene)=>["research","knowledge"].includes(scene.id));
  return <aside className="sidebar">
    <div className="side-toolbar"><div className="lights" aria-label="Mac 窗口装饰"><i/><i/><i/></div><div className="tools">
      <Button action="search" className="icon-button" aria-label="搜索介绍内容" title="搜索介绍内容" onClick={()=>setModal({kind:"search"})}><Icon name="search"/></Button>
      <Button action="new-crew" className="icon-button" aria-label="新建机组" title="新建机组" onClick={()=>setModal({kind:"newCrew"})}><Icon name="plus"/></Button>
      <Button action="refresh" className="icon-button" aria-label="刷新" title="刷新" onClick={()=>dispatch({type:"notice",text:"当前机组状态已刷新。"})}><Icon name="refresh"/></Button>
    </div></div>
    <div className="segmented side-mode">{(["hierarchy","timeline"] as const).map((mode)=><Button key={mode} action="sidebar" data-mode={mode} className={ui.sidebar===mode?"active":""} onClick={()=>setUi({sidebar:mode})}>{mode==="hierarchy"?"通讯录":"消息"}</Button>)}</div>
    <div className="side-list">
      {ui.sidebar==="hierarchy" ? <>
        <div className="machine-row"><Disclosure open={!ui.machineClosed} label="本机" onClick={()=>setUi({machineClosed:!ui.machineClosed})}/><span>本机</span></div>
        {!ui.machineClosed && <>
          <div className="crew-root"><Disclosure open={!closed.includes("crew-product")} label="PendingCrew" onClick={()=>dispatch({type:"toggleCrew",id:"crew-product"})}/><span>PendingCrew</span></div>
          {!closed.includes("crew-product") && scenes.filter((s)=>!["research","knowledge"].includes(s.id)).map((scene)=><CrewRow key={scene.id} scene={scene} indent={12}>{scene.id==="trip"&&collaborationChildren.length?<>{collaborationChildren.map((child)=><CrewRow key={child.id} scene={child} indent={24}/>)}</>:undefined}</CrewRow>)}
        </>}
      </> : scenes.map((scene)=><CrewRow key={scene.id} scene={scene}/>)}
    </div>
    <QuotaFooter/><ResizeHandle name="sidebar"/>
  </aside>;
}
