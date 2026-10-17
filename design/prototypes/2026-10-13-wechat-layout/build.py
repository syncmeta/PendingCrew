from sketch_helpers import *
import re
tabs=['消息','Todo','文档','进展']
states=[(m,t) for m in ['通讯录','消息'] for t in tabs]+[('消息','单聊'),('通讯录','单聊'),('消息','Todo详情'),('消息','答复后'),('消息','文档详情'),('消息','进展详情'),('消息','其他机组'),('通讯录','折叠层级')]
names=[f'{i+1:02d} {m} · {t}' for i,(m,t) in enumerate(states)]
boards={n:base('Artboard',n,(i%4)*1380,(i//4)*880,1280,800) for i,n in enumerate(names)}
key={v:n for v,n in zip(states,names)};links=[]
def add(b,a):b['layers'].append(a);return a
def link(a,b,label,target):
 a['flow']={'_class':'MSImmutableFlowConnection','destinationArtboardID':boards[target]['do_objectID'],'animationType':0};links.append((b['name'],label,target))
def button(b,label,x,y,w,target=None,active=False):
 a=add(b,rect(label,x,y,w,36,'#dcebe5' if active else '#f3f4f4',5));add(b,text(label,x+12,y+8,w-20,14,h=25))
 if target:link(a,b,label,target)
def label(b,s,x,y,w=700,size=16,color='#263238',h=40):return add(b,text(s,x,y,w,size,color,h))
def side_row(b,s,y,target,sub='',indent=0,selected=False):
 a=add(b,rect(s,10,y,300,68,'#dcebe5' if selected else '#f0f2f2',5));add(b,rect('谱系色条',22+indent,y+11,3,43,'#668f82'));label(b,s,34+indent,y+9,258-indent,15)
 if sub:label(b,sub,34+indent,y+36,258-indent,12,'#647477',25)
 if target:link(a,b,s,target)
def bubble(b,s,y,sender='后端',own=False,target=None):
 x=686 if own else 394;w=520 if own else 630
 avatarx=1218 if own else 348
 add(b,rect('头像：'+sender,avatarx,y,34,34,'#d6e0dc',5));label(b,sender[:1],avatarx+9,y+8,23,14,h=23)
 label(b,sender,x,y-27,w,13,'#697578',24)
 a=add(b,rect('消息：'+s,x,y,w,62,'#bce0cc' if own else '#ffffff',7));label(b,s,x+16,y+15,w-30,16,h=36)
 if target:link(a,b,'消息：'+s,target)
def contentrow(b,title,sub,y,target=None):
 a=add(b,rect(title,348,y,900,86,'#f6f7f7',6));label(b,title,366,y+13,857,17);label(b,sub,366,y+49,857,13,'#667578',26)
 if target:link(a,b,title,target)
for n,(mode,view) in zip(names,states):
 b=boards[n];b.update(hasBackgroundColor=True,includeBackgroundColorInExport=True,backgroundColor=color('#ffffff'),isFlowHome=n==key[('消息','消息')])
 add(b,rect('画布',0,0,1280,800,'#ffffff'));add(b,rect('左 sidebar',0,0,320,800,'#f0f2f2'));add(b,rect('右侧机组',320,0,960,800,'#f8f9f9'))
 label(b,'PendingCrew',18,16,240,19);label(b,'搜索机组',22,56,272,13,'#778285');add(b,rect('搜索背景',14,51,291,33,'#e5e9e9',5));label(b,'搜索机组',26,59,240,13,'#778285')
 currenttab=view if view in tabs else 'Todo' if view in ['Todo详情','答复后'] else '文档' if view=='文档详情' else '进展' if view=='进展详情' else '消息'
 for i,m in enumerate(['通讯录','消息']):button(b,m,14+i*149,96,142,key[(m,currenttab)],m==mode)
 if mode=='通讯录':
  button(b,'▾ 本机',14,148,291,key[('通讯录','折叠层级')])
  if view!='折叠层级':
   side_row(b,'▾ PendingName',195,None,'顶层机组 · 2 个子机组')
   side_row(b,'▾ PendingCrew',269,key[('通讯录',currenttab)],'后端：正在接工作工具',20,True)
   side_row(b,'后端实现',343,key[('消息','其他机组')],'Codex · 进行中',40)
   side_row(b,'前端设计',417,None,'你正在设计',40)
   side_row(b,'PendingBot',491,None,'最近无新消息',20)
  else:label(b,'本机层级已折叠',26,209,268,14,'#667578');button(b,'展开本机',22,250,270,key[('通讯录','消息')])
  button(b,'▸ 远端设备',14,588,291)
 else:
  side_row(b,'PendingCrew',153,key[('消息',currenttab)],'后端：工作工具验收通过 · 12:10',0,view!='其他机组')
  side_row(b,'后端实现',227,key[('消息','其他机组')],'检查点仍在继续 · 12:05',0,view=='其他机组')
  side_row(b,'前端设计',301,None,'你：保留原有聊天布局 · 11:58')
  side_row(b,'PendingName',375,None,'机长：继续推进 · 昨天')
  side_row(b,'PendingBot',449,None,'最近没有新消息 · 昨天')
  label(b,'最近活动倒序 · 来源：PendingName',22,543,283,12,'#718083')
 label(b,'已隐藏的群 ▸',24,691,270,13,'#6e7d7d');label(b,'Codex 额度：合成示例',24,727,270,12,'#6e7d7d');label(b,'本机 · 原型 / 合成数据',24,758,270,12,'#6e7d7d')
 label(b,'后端实现' if view=='其他机组' else 'PendingCrew',348,22,710,23);label(b,'机组设置 ···',1120,28,140,14,'#617471')
 for i,t in enumerate(tabs):button(b,t,348+i*112,73,102,key[(mode,t)],t==currenttab)
 add(b,rect('分隔线',320,126,960,1,'#e4e8e8'))
 if currenttab=='消息':
  button(b,'群聊',348,141,92,key[(mode,'消息')],view!='单聊');button(b,'单聊 · 后端',451,141,144,key[(mode,'单聊')],view=='单聊')
  label(b,'成员：你 · 机长 · 后端（Codex）',660,152,555,13,'#637777')
  add(b,rect('聊天画布',320,189,960,421,'#f1f3f3'));label(b,'今天 12:10',753,208,210,12,'#788482')
  if view=='单聊':bubble(b,'这部分先保持简洁，详情按需展开。',288,'你',True);bubble(b,'收到。相关进展仍记录在机组的同份工作里。',400);bubble(b,'查看 W12 的进展',500,target=key[('消息','进展详情')])
  elif view=='其他机组':bubble(b,'检查点验证继续进行。',294);bubble(b,'需要答复时沿用这个机组的 Todo。',406)
  else:bubble(b,'先沿用原来的整体形态，像微信一样。',288,'你',True);bubble(b,'界面分成消息、Todo、文档、进展四个视图。',400,'机长');bubble(b,'#21 等你回复：是否继续当前工作？',508,target=key[('消息','Todo详情')])
  add(b,rect('输入区',320,611,960,189,'#ffffff'));label(b,'附件   @   Todo',348,630,680,15,'#5f7670');label(b,'输入消息…',348,677,830,15,'#8a9693');button(b,'发送',1130,736,118,key[(mode,'消息')],True)
 elif currenttab=='Todo':
  label(b,'Todo',348,150,800,22);button(b,'全部',348,194,98,key[(mode,'Todo')],True);button(b,'等我回复',457,194,128,key[('消息','Todo详情')]);label(b,'新建 Todo ＋',1105,198,150,14,'#456f61')
  if view in ['Todo详情','答复后']:
   contentrow(b,'?  #21 · 是否继续当前工作？','来自后端 · 等你回复' if view=='Todo详情' else '来自后端 · 已答复',256)
   label(b,'请确认：继续完成工具验收后推进检查点。',369,365,855,18)
   add(b,rect('Todo 答复',366,428,850,118,'#ffffff',6));label(b,'继续推进，先完成工具验收。',385,450,813,16)
   button(b,'提交答复' if view=='Todo详情' else '答复已提交',368,575,164,key[('消息','答复后')],True);button(b,'返回 Todo',551,575,158,key[(mode,'Todo')]);label(b,'关联进展 W12',368,649,500,15,'#537768')
  else:
   contentrow(b,'?  #21 · 是否继续当前工作？','等你回复 · 后端提出 · 查看完整详情',256,key[('消息','Todo详情')])
   contentrow(b,'◉  #20 · 完成 Codex 首版工作跟进','进行中 · 原始需求原文保留 · 关联 W12',356,key[('消息','进展详情')])
   contentrow(b,'○  #19 · 前端设计保持原有形态','待办 · 新建项在最上面，同状态用圈形区分',456)
   contentrow(b,'●  #18 · 验证重连不重放消息','已完成 · 正文变灰，不加删除线',556)
 elif currenttab=='文档':
  label(b,'机组文档',348,153,800,22);label(b,'同机组内的文件与说明',348,192,850,14,'#778582')
  if view=='文档详情':
   contentrow(b,'codex-first.md','PendingCrew / 文档 · 原型内容',245)
   label(b,'Codex 首版验收',366,360,825,23);label(b,'1. 消息正常收发，恢复后不重放。',366,413,825,17);label(b,'2. Todo 答复回到原机组。',366,467,825,17);label(b,'3. 进展引用同一份工作记录。',366,521,825,17);button(b,'返回文档',366,634,166,key[(mode,'文档')])
  else:
   contentrow(b,'codex-first.md','首版验收说明 · 后端更新于 12:08',245,key[('消息','文档详情')]);contentrow(b,'interface-notes.md','界面设计讨论 · 你更新于 11:58',345,key[('消息','文档详情')]);contentrow(b,'reconnect-results.txt','重连回归记录 · 合成预览',445,key[('消息','文档详情')])
 elif currenttab=='进展':
  label(b,'机组进展',348,153,800,22);label(b,'引用与 Todo 关联的同份工作，不另建一份状态。',348,195,900,14,'#687a75')
  if view=='进展详情':
   contentrow(b,'W12 · 工作工具与检查点','负责人 后端 · 进行中 · 源需求 #20',246)
   label(b,'最新回报',368,372,800,18);label(b,'工具接口验收已完成，下一步接检查点跟进。',368,416,855,17);label(b,'下一次检查 · 今天 13:00（原型）',368,477,855,16);label(b,'备注 · 界面继续使用旧的聊天结构。',368,534,855,16);button(b,'看源 Todo',368,622,170,key[('消息','Todo')]);button(b,'返回进展',558,622,170,key[(mode,'进展')])
  else:contentrow(b,'W12 · 工作工具与检查点','进行中 · 后端 · 关联 #20 · 下次检查 13:00',245,key[('消息','进展详情')]);contentrow(b,'W13 · 微信式前端原型','进行中 · 你在设计 · 关联 #19',345,key[('消息','进展详情')]);contentrow(b,'W8 · 旧机组历史工作（只读）','所属机组已移除，默认排除出我的工作 · 原型示例',445)
page=default(json.loads((P/'schema-source/package/dist/page.schema.json').read_text()));page.update(name='PendingCrew · 微信式布局',isVisible=True,booleanOperation=-1,resizingConstraint=63,layers=list(boards.values()))
doc=default(schema['properties']['document']);doc['pages']=[page]
meta=obj('Meta');meta.update(version=146,appVersion='2026.3',build=233959,commit='prototype',pagesAndArtboards={page['do_objectID']:{'name':page['name'],'artboards':{b['do_objectID']:{'name':n} for n,b in boards.items()}}});meta['created'].update(version=146,appVersion='2026.3',build=233959)
document_file=dict(doc);document_file['pages']=[{'_class':'MSJSONFileReference','_ref_class':'MSImmutablePage','_ref':'pages/'+page['do_objectID']}]
with zipfile.ZipFile(P/'PendingCrew-WeChatLayout.sketch','w',zipfile.ZIP_DEFLATED) as z:
 for name,value in [('document.json',document_file),('meta.json',meta),('user.json',obj('User')),('pages/'+page['do_objectID']+'.json',page)]:z.writestr(name,json.dumps(value,ensure_ascii=False))
(P/'prototype-map.json').write_text(json.dumps({'artboards':len(boards),'links':links,'start':key[('消息','消息')]},ensure_ascii=False,indent=2))
panels=[]
for n,b in boards.items():
 body=[]
 for a in b['layers']:
  if 'flow' in a:
   f=a['frame'];target=next(k for k,v in boards.items() if v['do_objectID']==a['flow']['destinationArtboardID'])
   body.append('<button class="hotspot" aria-label="'+html.escape(a['name'],quote=True)+'" data-target="'+html.escape(target,quote=True)+'" style="left:'+str(f['x']/12.8)+'%;top:'+str(f['y']/8)+'%;width:'+str(f['width']/12.8)+'%;height:'+str(f['height']/8)+'%"></button>')
 panels.append('<section data-board="'+n+'" hidden><img alt="'+n+'" src="exports/'+n+'.png">'+''.join(body)+'</section>')
preview='''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>PendingCrew 微信式原型</title><style>body{margin:0;background:#dce1e0;font:14px system-ui}header{display:flex;gap:14px;padding:12px 22px;align-items:center}button,select{font:inherit;padding:8px 12px}main{max-width:1280px;margin:auto}section{position:relative}img{display:block;width:100%}.hotspot{position:absolute;border:0;background:transparent;cursor:pointer;padding:0}.hotspot:hover,.hotspot:focus-visible{background:#00695c18;outline:2px solid #487d73}section[hidden]{display:none}small{color:#526963}</style><header><strong>PendingCrew · 微信式草图</strong><button id="back">返回</button><select id="jump" aria-label="画板"></select><small>合成数据 / 原型；编辑请用 Sketch。</small></header><main>'''+''.join(panels)+'''</main><script>const sections=[...document.querySelectorAll('section')],jump=document.querySelector('#jump'),history=[];for(const s of sections){const o=document.createElement('option');o.textContent=s.dataset.board;jump.append(o)}function show(n,push=true){const old=sections.find(s=>!s.hidden);if(push&&old)history.push(old.dataset.board);for(const s of sections)s.hidden=s.dataset.board!==n;jump.value=n}document.addEventListener('click',e=>{if(e.target.dataset.target)show(e.target.dataset.target)});jump.addEventListener('change',()=>show(jump.value));document.querySelector('#back').onclick=()=>{const n=history.pop();if(n)show(n,false)};show('05 消息 · 消息',false);</script></html>'''
(P/'index.html').write_text(preview)
print('Created',len(boards),'artboards',len(links),'native Flow links')
