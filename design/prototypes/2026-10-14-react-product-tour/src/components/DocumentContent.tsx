import { productText } from "../domain/displayText";
import { MessageText } from "./MessageText";
const segment=(value:string)=>value.replace(/\s/g,"-").replace(/&/g,"-and-").replace(/%/g,"-percent").replace(/[?#<>:"|*]/g,"").toLowerCase();
function Inline({text}:{text:string}) {
 return <>{text.split(/(\[\[[^\]]+\]\]|\[[^\]]+\]\(https?:\/\/[^)]+\))/g).map((part,i)=>{
  if(part.startsWith("[[")){const [path,label]=part.slice(2,-2).split("|");return <a key={i} href={"https://docs.pendingname.com/"+path.split("/").map(segment).join("/")} target="_blank" rel="noopener noreferrer">{productText(label??path.split("/").at(-1)??"")}</a>;}
  const match=part.match(/^\[([^\]]+)\]\((https?:\/\/[^)]+)\)$/);
  if(match)return <a key={i} href={match[2]} target="_blank" rel="noopener noreferrer">{productText(match[1])}</a>;
  return <MessageText key={i} text={part}/>;
 })}</>;
}
/** Render document-site Markdown as React nodes, with offline copies of embedded images. */
export function DocumentContent({body,assets={}}:{body:string;assets?:Record<string,string|undefined>}) {
 return <div className="document-content">{body.split(/\n\s*\n/).map((block,index)=>{
  const embed=block.startsWith("![[")&&block.endsWith("]]" )?block.slice(3,-2).split("|"):null;
  const image=embed?["",embed[0],embed.slice(1).join("|")]:null;
  if(image)return assets[image[1]]?<figure key={index}><img src={assets[image[1]]} alt={image[2]??image[1]}/>{image[2]&&<figcaption>{image[2]}</figcaption>}</figure>:<p key={index}>{image[2]??image[1]}</p>;
  if(block.startsWith("```"))return <pre key={index}><code>{block.replace(/^```[^\n]*\n?/,"").replace(/```$/,"")}</code></pre>;
  const heading=block.match(/^#{1,6}\s+([^\n]+)$/);if(heading)return <h4 key={index}><Inline text={heading[1]}/></h4>;
  const lines=block.split("\n");const firstBullet=lines.findIndex((line)=>/^[-*] /.test(line));
  if(firstBullet>=0)return <div key={index}>{firstBullet>0&&<p><Inline text={lines.slice(0,firstBullet).join("\n")}/></p>}<ul>{lines.slice(firstBullet).map((line,i)=><li key={i}><Inline text={line.replace(/^[-*] /,"")}/></li>)}</ul></div>;
  return <p key={index}><Inline text={block}/></p>;
 })}</div>;
}
