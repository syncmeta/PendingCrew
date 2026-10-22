import { useState } from "react";
import claude from "../assets/claude.svg";
import openai from "../assets/openai.svg";
import { Button, Icon } from "./primitives";
interface Ring {id:string;brand:string;caption:string;used:number;reset:string;wide?:boolean}
const rings:Ring[] = [
  {id:"claude-5h",brand:"Claude Code",caption:"5h",used:32,reset:"4 小时后重置"},
  {id:"claude-week",brand:"Claude Code",caption:"周",used:58,reset:"3 天后重置"},
  {id:"claude-model",brand:"Claude Code",caption:"Fable",used:46,reset:"3 天后重置",wide:true},
  {id:"codex-week",brand:"Codex",caption:"周",used:24,reset:"5 天后重置"},
];
export function QuotaFooter() {
  const [hovered,setHovered]=useState<Ring|null>(null);
  const [refreshed,setRefreshed]=useState(false);
  return <section className="side-footer quota-footer" aria-label="订阅额度（演示数据）">
    {[{brand:"Claude Code",icon:claude},{brand:"Codex",icon:openai}].map(({brand,icon})=><div className="quota-brand-row" key={brand}>
      <img className="quota-brand-mark" src={icon} alt={brand} title={brand}/>
      {rings.filter((ring)=>ring.brand===brand).map((ring)=><div role="img" tabIndex={0} key={ring.id} className={"native-quota-ring "+(ring.wide?"wide":"")} aria-label={`${brand} ${ring.caption} 已用 ${ring.used}%，剩余 ${100-ring.used}%（演示）`} title={`演示数据 · 已用 ${ring.used}%，剩余 ${100-ring.used}%`} onMouseEnter={()=>setHovered(ring)} onMouseLeave={()=>setHovered(null)} onFocus={()=>setHovered(ring)} onBlur={()=>setHovered(null)}>
        <svg viewBox={`0 0 ${ring.wide?54:33} 33`} aria-hidden="true">
          {ring.wide?<><path className="quota-track" d="M27 1.5H37.5a15 15 0 0 1 0 30h-21a15 15 0 0 1 0-30Z"/><path className="quota-value" pathLength="100" strokeDasharray={`${ring.used} 100`} d="M27 1.5H37.5a15 15 0 0 1 0 30h-21a15 15 0 0 1 0-30Z"/></>:<><circle className="quota-track" cx="16.5" cy="16.5" r="15"/><circle className="quota-value" cx="16.5" cy="16.5" r="15" pathLength="100" strokeDasharray={`${ring.used} 100`} transform="rotate(-90 16.5 16.5)"/></>}
        </svg><span className="quota-caption">{ring.caption}</span><span className="quota-percent">{ring.used}<small>%</small></span>
      </div>)}
    </div>)}
    <div className="quota-freshness"><Button action="refresh-demo-quota" aria-label="刷新演示额度" title="演示额度，不读取账户" onClick={()=>setRefreshed(true)}><Icon name="refresh"/></Button><span>{hovered?`${hovered.brand} ${hovered.caption} · ${hovered.reset}`:`演示额度 · ${refreshed?"已刷新":"刚刚"}`}</span></div>
  </section>;
}
