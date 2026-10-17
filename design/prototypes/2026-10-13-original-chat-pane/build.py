from sketch_helpers import *

tabs=['消息','Todo','文档','进展']
states=['群聊','单聊','通讯录','通讯录折叠','Todo','Todo详情','答复后','文档','文档详情','进展','步骤展开','步骤折叠','产物','消息分组','Todo分组','文档分组','进展分组','分组管理','新建分组','重命名分组','移动内容','移动完成','空文件夹','搜索','成员详情','检查点展开','工具折叠','Todo20详情','时间流','Todo收起','成员收起','辅助栏收起','讨论群']
boards={s:base('Artboard',f'{i+1:02d} {s}',(i%5)*1500,(i//5)*940,1440,860) for i,s in enumerate(states)}
links=[]
def add(b,a): b['layers'].append(a);return a
def label(b,s,x,y,w=600,size=14,c='#1b1a14',h=28):return add(b,text(s,x,y,w,size,c,h))
def box(b,s,x,y,w,h=36,fill='#ffffff',target=None):
 a=add(b,rect(s,x,y,w,h,fill,5))
 if target:
  a['flow']={'_class':'MSImmutableFlowConnection','destinationArtboardID':boards[target]['do_objectID'],'animationType':0};links.append([b['name'],s,boards[target]['name']])
 return a
def btn(b,s,x,y,w=110,target=None,active=False):
 box(b,s,x,y,w,32,'#e3eeea' if active else '#efeee9',target);label(b,s,x+10,y+7,w-16,13)
def row(b,s,sub,x,y,w,target=None,selected=False,h=70):
 box(b,s,x,y,w,h,'#e3eeea' if selected else '#ffffff',target);label(b,s,x+12,y+10,w-24,14);label(b,sub,x+12,y+38,w-24,12,'#6e6a5c')
GLYPHS=['🦊','🐼','🐯','🦁','🐸','🐧','🐳','🐙','🦉','🦄','🐝','🦋','🐢','🐬','🦒','🦔','🌵','🌻','🌸','🌙','⭐','🔥','🍄','🍀','🍓','🍑','🍋','🍇','🥑','🌽','🥨','🍪','🎈','🎨','🎭','🎪','🎲','🧩','🪁','🪐','🚀','⛵','🏕','🪴','🪨','💎','🧊','🦤']
MASK=(1<<64)-1
def pick(seed):
 h=1469598103934665603
 for c in seed: h=((h^ord(c))*1099511628211)&MASK
 chosen=[]
 while len(chosen)<3:
  g=GLYPHS[h%len(GLYPHS)]
  if g not in chosen: chosen.append(g)
  h=(h*6364136223846793005+1442695040888963407)&MASK
 return chosen
def circle(b,name,x,y,size,fill):
 a=base('Oval',name,x,y,size,size);a['isClosed']=True;a['style']=rect('style',0,0,1,1,fill)['style']
 for point,frm,to in [('{0.5, 0}','{0.2238576, 0}','{0.7761424, 0}'),('{1, 0.5}','{1, 0.2238576}','{1, 0.7761424}'),('{0.5, 1}','{0.7761424, 1}','{0.2238576, 1}'),('{0, 0.5}','{0, 0.7761424}','{0, 0.2238576}')]:
  c=obj('CurvePoint');c.update(point=point,curveFrom=to,curveTo=frm,hasCurveFrom=True,hasCurveTo=True,curveMode=2);a['points'].append(c)
 return add(b,a)
def avatar(b,seed,x,y,size=36,group=True):
 circle(b,'头像边框',x,y,size,'#e2e0d7')
 circle(b,'头像底色',x+1,y+1,size-2,'#ffffff' if group else '#e3eeea')
 if group:
  r=size*.18;gsize=size*.32
  for g,(dx,dy) in zip(pick(seed),[(0,-r),(-r*.866,r*.5),(r*.866,r*.5)]):label(b,g,x+size/2+dx-gsize/2,y+size/2+dy-gsize*.7,gsize+3,gsize,h=gsize*1.6)
 else:label(b,'🦊' if seed=='backend' else '🦉',x+size*.18,y+size*.12,size*.75,size*.55,h=size*.8)
def conv(b,title,sub,y,target,selected=False,group=True,seed='crew-main'):
 box(b,title,301,y,218,74,'#e3eeea' if selected else '#ffffff',target)
 avatar(b,seed,313,y+17,36,group);label(b,title,357,y+14,150,14);label(b,sub,357,y+40,150,12,'#6e6a5c')
def crewrow(b,title,sub,y,indent=0,selected=False,target='群聊'):
 box(b,title,10,y,280,72,'#e3eeea' if selected else '#efeee9',target)
 box(b,'谱系色条',26+indent,y+9,3,51,'#044735');label(b,title,36+indent,y+9,241-indent,14);label(b,sub,36+indent,y+36,241-indent,12,'#6e6a5c')
def bubble(b,s,y,sender='机长',own=False,target=None):
 end=1408 if state=='辅助栏收起' else 1083
 w=min(416,end-607);x=end-w-40 if own else 587
 avatar(b,'backend' if sender=='后端' else 'captain',end-30 if own else 547,y,30,False)
 label(b,sender,x,y-24,w,12,'#6e6a5c');box(b,s,x,y,w,62,'#e3eeea' if own else '#efeee9',target);label(b,s,x+14,y+15,w-28,14,h=42)

for state,b in boards.items():
 b.update(hasBackgroundColor=True,includeBackgroundColorInExport=True,backgroundColor=color('#ffffff'),isFlowHome=state=='群聊')
 box(b,'窗口',0,0,1440,860);box(b,'sidebar',0,0,300,860,'#efeee9')
 label(b,'● ● ●',18,14,130,13,'#6e6a5c');btn(b,'⌕',152,12,42,'搜索');btn(b,'＋',200,12,42,'新建分组');btn(b,'↻',248,12,42,'群聊')
 timeline=state=='时间流'
 btn(b,'通讯录',10,58,138,'通讯录',not timeline);btn(b,'消息',152,58,138,'时间流',timeline)
 if timeline:
  crewrow(b,'PendingCrew','机长：工具检查完成 · 12:10',105,selected=True)
  crewrow(b,'后端实现','检查点继续进行 · 12:05',183)
  crewrow(b,'前端设计','保留原有布局 · 昨天',261)
  crewrow(b,'PendingName','继续推进 · 昨天',339)
  crewrow(b,'PendingBot','最近没有新消息 · 昨天',417)
  label(b,'PendingName / 本机',36,506,240,12,'#6e6a5c')
 else:
  btn(b,'▸ 本机' if state=='通讯录折叠' else '▾ 本机',10,103,280,'通讯录' if state=='通讯录折叠' else '通讯录折叠')
  if state!='通讯录折叠':
   crewrow(b,'▾ PendingName','继续推进 · 昨天',147)
   crewrow(b,'▾ PendingCrew','机长：工具检查完成 · 12:10',225,18,True)
   crewrow(b,'后端实现','检查点继续进行 · 12:05',303,36)
   crewrow(b,'前端设计','保留原有布局 · 昨天',381,36)
   crewrow(b,'▾ 产品讨论','另一父关系',459,18)
   crewrow(b,'前端设计 ↗','同一机组 · 多父引用',537,36)
  btn(b,'▸ 远端设备',10,638,280,'通讯录')
 label(b,'已隐藏的群 ▸',20,731,260,13,'#6e6a5c');label(b,'Codex 额度 · —',20,775,260,12,'#6e6a5c');label(b,'本机 · 设置',20,818,260,12,'#6e6a5c')
 tab='Todo' if state in ['Todo','Todo详情','Todo20详情','答复后','Todo分组'] else '文档' if state in ['文档','文档详情','文档分组','产物'] else '进展' if state in ['进展','步骤展开','步骤折叠','进展分组','检查点展开','工具折叠'] else '消息'
 label(b,'PendingCrew',324,25,620,20)
 label(b,'本机 / PendingName',324,57,620,12,'#6e6a5c')
 btn(b,'搜索当前内容',1110,27,160,'搜索');btn(b,'···',1280,27,128,'分组管理')
 for i,t in enumerate(tabs):btn(b,t,324+i*104,91,94,'群聊' if t=='消息' else t,t==tab)
 box(b,'内容分隔线',300,136,1140,1,'#e2e0d7')
 if tab=='消息':
  box(b,'会话选择',300,137,220,723,'#ffffff');box(b,'会话分隔线',520,137,1,723,'#e2e0d7')
  btn(b,'全部会话 ▾',311,149,196,'消息分组')
  conv(b,'本群','机长：等你答复 · 12:10',194,'群聊',state not in ['单聊','讨论群'])
  conv(b,'后端','验证记录已整理 · 12:08',272,'单聊',state=='单聊',False,'backend')
  conv(b,'实现讨论','后端：继续检查点 · 12:05',350,'讨论群',state=='讨论群',True,'crew-discussion')
  conv(b,'设计','保留原有布局 · 昨天',428,'单聊',False,False,'design')
  label(b,'当前机组的会话',315,811,192,12,'#6e6a5c')
  hidden=state=='辅助栏收起';edge=1440 if hidden else 1110;cw=edge-521
  box(b,'聊天画布',521,137,cw,723,'#ffffff')
  label(b,'后端' if state=='单聊' else '实现讨论' if state=='讨论群' else '本群',541,150,260,16)
  btn(b,'展开 Todo / 成员' if hidden else '收起侧面板',edge-161,146,144,'群聊' if hidden else '辅助栏收起')
  btn(b,'人类沟通',541,188,112,'群聊',True);btn(b,'仅@你',663,188,80,'搜索');btn(b,'消息文件夹 ▾',753,188,158,'消息分组')
  label(b,'今天 12:10',740,239,200,12,'#6e6a5c')
  if state=='单聊':
   bubble(b,'工具检查的记录放在哪个文件夹？',307,'你',True)
   bubble(b,'文档 / 验证记录；讨论放在「实现讨论」。',419,'后端')
   bubble(b,'reconnect-results.txt · 点击查看',531,'后端',target='文档详情')
  elif state=='讨论群':
   bubble(b,'这个群集中讨论工具和检查点。',307,'机长')
   bubble(b,'W12 · 点击展开步骤和产物',419,'后端',target='步骤展开')
  else:
   bubble(b,'继续保留原来的样式与 sidebar。',307,'你',True)
   bubble(b,'会话放在消息里，Todo 和成员可以收起。',419)
   bubble(b,'? #21 等你答复：是否继续当前工作？',531,'后端',target='Todo详情')
  box(b,'输入分隔线',521,664,cw,1,'#e2e0d7');box(b,'输入区',521,665,cw,195)
  label(b,'＋  附件    @    Todo    引用',541,683,cw-40,14,'#6e6a5c');label(b,'输入消息…',541,729,cw-40,14,'#6e6a5c');btn(b,'发送',edge-132,806,110,'群聊',True)
  if not hidden:
   todoClosed=state=='Todo收起';membersClosed=state=='成员收起'
   box(b,'右栏分隔线',1110,137,1,723,'#e2e0d7');box(b,'右栏',1111,137,329,723,'#ffffff')
   btn(b,('▸' if todoClosed else '▾')+' 本群 Todo',1125,151,196,'群聊' if todoClosed else 'Todo收起');btn(b,'详细',1331,151,92,'Todo')
   if not todoClosed:
    label(b,'Agent 的  ·  人类的',1132,196,280,12,'#6e6a5c')
    row(b,'? #21 是否继续当前工作？','等你答复 · 后端',1125,233,298,'Todo详情')
    row(b,'◉ #20 Codex 首版跟进','进行中 · 机长',1125,313,298,'Todo20详情')
    row(b,'○ #19 保留原有前端形态','待办 · 设计',1125,393,298,'Todo详情')
   memberY=211 if todoClosed else 500
   box(b,'面板分隔线',1111,memberY-12,329,1,'#e2e0d7');btn(b,('▸' if membersClosed else '▾')+' 本群成员  4',1125,memberY,298,'群聊' if membersClosed else '成员收起')
   if not membersClosed:
    for i,(n,sub) in enumerate([('你','人类'),('机长','Codex · 正在整理进展'),('后端','Codex · 正在工作'),('设计','Codex · 等待消息')]):row(b,n,sub,1125,memberY+48+i*70,298,'单聊' if i>0 else '成员详情',h=64)
 else:
  # Each view has an independent folder rail, unrelated to the crew DAG.
  box(b,'文件夹栏',300,137,220,723,'#efeee9');label(b,'文件夹',320,158,140,15);btn(b,'＋',458,152,44,'新建分组')
  folders={'Todo':['全部 Todo','首版验收','前端设计','等我决策','未分组'],'文档':['全部文档','设计说明','验证记录','接口约定','未分组'],'进展':['全部进展','首版实现','界面迭代','历史归档','未分组']}[tab]
  for i,f in enumerate(folders):btn(b,('▾ ' if i==1 else '▸ ' if i in [2,3] else '')+f,316,210+i*48,188,'空文件夹' if f=='未分组' else tab,i==1)
  btn(b,'管理分组',316,783,188,'分组管理')
  label(b,{'Todo':'首版验收','文档':'验证记录','进展':'首版实现'}[tab],548,160,680,20)
  btn(b,'文件夹 ▾',1250,155,158,tab+'分组');label(b,'PendingCrew / '+tab+' / 当前文件夹',548,197,760,12,'#6e6a5c')
  if tab=='Todo':
   btn(b,'Agent 的',548,233,110,'Todo',True);btn(b,'人类的',668,233,110,'Todo');btn(b,'等我答复',788,233,116,'Todo详情');btn(b,'移动…',1288,233,120,'移动内容')
   if state in ['Todo详情','Todo20详情','答复后']:
    row(b,'◉ #20 Codex 首版跟进' if state=='Todo20详情' else '? #21 是否继续当前工作？','源 Todo · 后端提出 · '+('已答复' if state=='答复后' else '等你答复'),548,285,860)
    label(b,'先完成工具验收，再推进检查点。请确认是否继续。',564,380,820,17)
    row(b,'后端 · 12:10','工具验收记录已经补齐，关联进展 W12。',548,435,860,'步骤展开')
    row(b,'你的答复','继续推进，先完成工具验收。',548,537,860)
    btn(b,'答复已提交' if state=='答复后' else '提交答复',548,636,160,'答复后',True);btn(b,'返回 Todo',721,636,140,'Todo');btn(b,'查看关联进展',873,636,160,'步骤展开')
   else:
    for i,(s,sub,target) in enumerate([('? #21 是否继续当前工作？','等你答复 · 后端 · 12:10','Todo详情'),('◉ #20 Codex 首版跟进','进行中 · 关联 W12','步骤展开'),('○ #19 保留原有前端形态','待办 · 设计','Todo详情'),('● #18 验证重连不重放','已完成 · 后端','Todo详情')]):row(b,s,sub,548,291+i*100,860,target)
  elif tab=='文档':
   btn(b,'移动…',1288,233,120,'移动内容')
   if state in ['文档详情','产物']:
    label(b,'reconnect-results.txt',548,272,800,22);label(b,'后端更新 · 今天 12:08 · v3',548,311,800,12,'#6e6a5c')
    label(b,'重连检查记录',548,374,800,20);label(b,'连接断开后再次接入，保留同一条已发送消息。',548,421,800,16)
    label(b,'检查项',548,486,800,17);label(b,'✓ 已发送消息只保留一次\n✓ 未发生额外 runner turn\n✓ 记录与关联步骤一起保留',548,526,800,16,h=110)
    btn(b,'关联进展 W12',548,696,170,'步骤展开');btn(b,'返回文档',730,696,130,'文档')
   else:
    for i,(s,sub) in enumerate([('reconnect-results.txt','后端 · 12:08 · 关联 W12 / 重连验证'),('tool-checks.md','后端 · 昨天 · 工具验收'),('first-release-notes.md','机长 · 昨天 · 首版说明')]):row(b,s,sub,548,290+i*100,860,'文档详情')
  else:
   btn(b,'移动…',1288,233,120,'移动内容')
   expanded=state!='步骤折叠'
   row(b,('▾' if expanded else '▸')+' W12 · 工作工具与检查点','进行中 · 负责人 后端 · 源 Todo #20 · 更新 12:10',548,280,860,'步骤折叠' if expanded else '步骤展开')
   if expanded:
    label(b,'目标   工具验收通过后接入检查点跟进',572,366,806,15)
    label(b,'负责人  后端        状态  进行中        下一步  检查点跟进',572,403,806,14,'#607568')
    row(b,('▸' if state=='工具折叠' else '▾')+' 01 工具验收   已完成','后端 · 完成 12:08 · 2 项检查 / 1 份产物',572,445,812,'步骤展开' if state=='工具折叠' else '工具折叠')
    if state!='工具折叠': row(b,'✓ 重连只保留一次消息','检查完成 · 证据与记录关联到本步骤',596,525,788,'产物',h=66)
    if state!='工具折叠': btn(b,'产物 · reconnect-results.txt',612,602,290,'产物');btn(b,'源 Todo #20',915,602,155,'Todo20详情')
    row(b,('▾' if state=='检查点展开' else '▸')+' 02 检查点跟进   进行中','后端 · 下一次检查 13:00 · 依赖 01',572,662 if state!='工具折叠' else 530,812,'步骤展开' if state=='检查点展开' else '检查点展开')
    if state=='检查点展开': row(b,'◉ 下一检查：保存并恢复工作上下文','负责人 后端 · 依赖工具检查通过',596,745,788,'Todo详情',h=70)
   else:
    row(b,'▸ W13 · 微信式前端原型','进行中 · 负责人 设计 · 源 Todo #19',548,382,860,'步骤展开')
    row(b,'▸ W8 · 历史工作','只读 · 原机组已移除',548,484,860,'步骤展开')
 if state in ['消息分组','Todo分组','文档分组','进展分组','分组管理','新建分组','重命名分组','移动内容','移动完成','空文件夹','搜索','成员详情']:
  box(b,'浮层背景',540,228,650,480,'#ffffff');label(b,{'新建分组':'新建文件夹','重命名分组':'重命名文件夹','移动内容':'移动到文件夹','移动完成':'已移动到「实现讨论」','空文件夹':'未分组','搜索':'搜索当前机组','成员详情':'成员 · 你'}.get(state,'文件夹 · '+tab),564,251,550,21)
  btn(b,'关闭',1080,245,86,'群聊')
  if state in ['新建分组','重命名分组']:
   label(b,'名称',564,320,500,14);row(b,'实现讨论','仅在当前内容视图内使用',564,357,600);btn(b,'保存',564,579,130,'分组管理',True)
  elif state in ['移动完成','空文件夹','成员详情']:
   label(b,'内容仍是同一份记录，引用保持有效。' if state=='移动完成' else '这里还没有内容。可以移动已有内容到这里。' if state=='空文件夹' else '当前机组成员 · 人类',564,342,600,16)
   btn(b,'返回消息',564,579,140,'群聊')
  elif state=='搜索':
   row(b,'检查点','消息 / Todo / 文档 / 进展',564,320,600)
   row(b,'W12 · 工作工具与检查点','进展 / 首版实现 · 后端',564,409,600,'步骤展开')
   row(b,'#21 是否继续当前工作？','Todo / 首版验收 · 等你答复',564,495,600,'Todo详情')
  else:
   for i,s in enumerate({'消息':['全部消息','▾ 实现讨论','   验证讨论','界面讨论'],'Todo':['全部 Todo','▾ 首版验收','   工具验收','前端设计'],'文档':['全部文档','▾ 验证记录','   重连记录','设计说明'],'进展':['全部进展','▾ 首版实现','   检查点','界面迭代']}[tab]):btn(b,s,564,315+i*46,420,'移动完成' if state=='移动内容' else '群聊' if tab=='消息' else tab,i==1)
   btn(b,'新建文件夹',564,535,170,'新建分组');btn(b,'重命名',746,535,130,'重命名分组');btn(b,'移动内容',888,535,150,'移动内容')

page=default(json.loads((P/'schema-source/package/dist/page.schema.json').read_text()));page.update(name='原样式 · 消息内会话与可收起面板',isVisible=True,booleanOperation=-1,resizingConstraint=63,layers=list(boards.values()))
doc=default(schema['properties']['document']);doc['pages']=[{'_class':'MSJSONFileReference','_ref_class':'MSImmutablePage','_ref':'pages/'+page['do_objectID']}]
meta=obj('Meta');meta.update(version=146,appVersion='2026.3',build=233959,commit='prototype',pagesAndArtboards={page['do_objectID']:{'name':page['name'],'artboards':{b['do_objectID']:{'name':b['name']} for b in boards.values()}}});meta['created'].update(version=146,appVersion='2026.3',build=233959)
with zipfile.ZipFile(P/'PendingCrew-OriginalChatPane.sketch','w',zipfile.ZIP_DEFLATED) as z:
 for n,v in [('document.json',doc),('meta.json',meta),('user.json',obj('User')),('pages/'+page['do_objectID']+'.json',page)]:z.writestr(n,json.dumps(v,ensure_ascii=False))
(P/'prototype-map.json').write_text(json.dumps({'artboards':len(boards),'links':links,'start':'群聊'},ensure_ascii=False,indent=2))
# Render the exact editable Sketch layers as HTML, without depending on raster exports.
panels=[]
for state,b in boards.items():
 parts=[]
 for a in b['layers']:
  f=a['frame'];style=f"left:{f['x']}px;top:{f['y']}px;width:{f['width']}px;height:{f['height']}px;"
  if a['_class']=='text':
   attr=a['attributedString']['attributes'][0]['attributes'];c=attr['MSAttributedStringColorAttribute'];rgb=','.join(str(round(c[k]*255)) for k in ['red','green','blue']);size=attr['MSAttributedStringFontAttribute']['attributes']['size']
   parts.append(f'<div class="txt" style="{style}font-size:{size}px;color:rgb({rgb})">{html.escape(a["attributedString"]["string"])}</div>')
  else:
   c=a['style']['fills'][0]['color'];rgb=','.join(str(round(c[k]*255)) for k in ['red','green','blue']);dest=next((k for k,v in boards.items() if a.get('flow',{}).get('destinationArtboardID')==v['do_objectID']),None)
   parts.append(f'<{"button" if dest else "div"} '+(f'data-target="{dest}" aria-label="{html.escape(a["name"],quote=True)}" ' if dest else '')+f'class="shape" style="{style}background:rgb({rgb});border-radius:{(f["width"]/2 if a["_class"]=="oval" else a.get("fixedRadius",5))}px"></'+('button' if dest else 'div')+'>')
 panels.append(f'<section data-board="{state}" hidden>'+''.join(parts)+'</section>')
(P/'index.html').write_text('''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>PendingCrew · 文件夹与聊天</title><style>body{margin:0;background:#dde2dc;font:14px -apple-system,BlinkMacSystemFont,"PingFang SC",sans-serif}header{height:48px;display:flex;align-items:center;gap:14px;padding:0 18px}button,select{font:inherit;cursor:pointer}main{width:1440px;margin:auto}section{position:relative;width:1440px;height:860px;box-shadow:0 10px 35px #52615322}section[hidden]{display:none}.shape,.txt{position:absolute;box-sizing:border-box;border:0;margin:0;padding:0}.txt{pointer-events:none;white-space:pre-wrap;line-height:1.5;overflow:hidden}button.shape:hover{outline:2px solid #659778;outline-offset:-2px}small{color:#637068}</style><header><strong>PendingCrew · 设计预览</strong><button id="back">返回</button><select id="jump" aria-label="画板"></select><small>合成数据 · 点击切换原型状态 · 文字可在 Sketch 编辑</small></header><main>'''+''.join(panels)+'''</main><script>const panels=[...document.querySelectorAll('section')],jump=document.querySelector('#jump'),trail=[];for(const s of panels){const o=document.createElement('option');o.textContent=s.dataset.board;jump.append(o)}function show(n,push=true){const old=panels.find(s=>!s.hidden);if(push&&old)trail.push(old.dataset.board);panels.forEach(s=>s.hidden=s.dataset.board!==n);jump.value=n}document.addEventListener('click',e=>{if(e.target.dataset.target)show(e.target.dataset.target)});jump.onchange=()=>show(jump.value);document.querySelector('#back').onclick=()=>{const n=trail.pop();if(n)show(n,false)};show('群聊',false);</script></html>''')
print(len(boards),'artboards;',len(links),'Flow links')
