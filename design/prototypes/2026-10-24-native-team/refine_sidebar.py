"""Refine the existing state graph into editable, native Mac-point Sketch layers.
No product code, source prototype, or original Sketch document is modified.
"""
from refined_helpers import *
import math, sys, hashlib
from copy import deepcopy
W,H=1280,820
INK='#202124';MUTED='#858585';SOFT='#b2b2b2';LINE='#e7e7e7';ACCENT='#044735';MINT='#e3eeea'
PROOF='--proof' in sys.argv
oldzip=zipfile.ZipFile(P/'PendingCrew-PrivateTeam.sketch');oldoc=json.loads(oldzip.read('document.json'));oldpage=json.loads(oldzip.read(oldoc['pages'][0]['_ref']+'.json'))
allstates=[b['name'][3:] for b in oldpage['layers']]+['读取中','读取失败','只读权限','提交结果未知','筛选为空','Codex执行','Codex历史','Codex设置','Codex高级设置','Codex请求答复','Codex控制不可用','Codex操作确认','会话选择·人待办','会话选择·AI待办','会话选择·文档','会话选择·进展','Codex输入选择','Codex删除确认','Codex截断确认','Codex排队回执','后端会话']
stateids={b['name'][3:]:b['do_objectID'] for b in oldpage['layers']}
for state in allstates:
 if state not in stateids:stateids[state]=uid()
states=['群聊','新建机组','人待办','文档数据'] if PROOF else allstates
boards={s:base('Artboard',f'{allstates.index(s)+1:02d} '+('会话选择' if s=='列表折叠' else s),i%4*1340,i//4*900,W,H) for i,s in enumerate(states)}
for s,b in boards.items():b.update(do_objectID=stateids[s],hasBackgroundColor=True,includeBackgroundColorInExport=True,backgroundColor=color('#ffffff'),isFlowHome=s=='群聊')
masters={};flows=[];overrides={}
def put(b,a):b['layers'].append(a);return a
def label(b,s,x,y,w=400,size=13,c=INK,h=24,weight='regular',name=None):
 a=text(s,x,y,w,size,c,h);a['name']=name or s
 if weight!='regular':a['attributedString']['attributes'][0]['attributes']['MSAttributedStringFontAttribute']['attributes']['name']='PingFangSC-'+('Semibold' if weight=='semibold' else 'Medium')
 return put(b,a)
def shape(b,n,x,y,w,h,c='#ffffff',r=0,border=None,shadow=False,alpha=1):
 a=rect(n,x,y,w,h,c,r);a['style']['fills'][0]['color']['alpha']=alpha
 if border:
  br=obj('Border');br['contextSettings']['opacity']=1;br.update(isEnabled=True,color=color(border),fillType=0,position=1,thickness=.65);a['style']['borders']=[br]
 if shadow:
  sh=obj('Shadow');sh['contextSettings']['opacity']=1;sh.update(isEnabled=True,blurRadius=12,offsetX=0,offsetY=3,spread=0,color=color('#000000'));sh['color']['alpha']=.055;a['style']['shadows']=[sh]
 return put(b,a)
def link(a,target):
 if target not in boards and target!='back':target='群聊' if PROOF else target
 if target not in boards and target!='back':raise ValueError(target)
 a['flow']={'_class':'MSImmutableFlowConnection','destinationArtboardID':'back' if target=='back' else boards[target]['do_objectID'],'animationType':0,'maintainScrollPosition':True}
 flows.append((a['name'],target));return a
def hotspot(b,n,x,y,w,h,target):return link(shape(b,n,x,y,w,h,alpha=0),target)
def path(b,n,pts,x,y,w,h,c=INK,width=1.55,closed=False):
 a=base('ShapePath',n,x,y,w,h);a.update(isClosed=closed,edited=True)
 for xx,yy in pts:
  p='{'+str(xx/24)+', '+str(yy/24)+'}';pt=obj('CurvePoint');pt.update(point=p,curveFrom=p,curveTo=p,curveMode=1);a['points'].append(pt)
 a['style']=obj('Style');br=obj('Border');br['contextSettings']['opacity']=1;br.update(isEnabled=True,color=color(c),fillType=0,position=0,thickness=width);a['style']['borders']=[br];return put(b,a)
def ellipse(b,n,x,y,w,h,c,fill=False,stroke=1.5):
 a=rect(n,x,y,w,h,c,min(w,h)/2)
 if not fill:
  a['style']['fills']=[];br=obj('Border');br['contextSettings']['opacity']=1;br.update(isEnabled=True,color=color(c),fillType=0,position=0,thickness=stroke);a['style']['borders']=[br]
 return put(b,a)
def icon_raw(b,k,x,y,size=20,c=INK):
 def p(n,pts,width=1.55):return path(b,k+'/'+n,pts,x,y,size,size,c,width)
 def e(n,xx,yy,ww,hh):return ellipse(b,k+'/'+n,x+xx*size/24,y+yy*size/24,ww*size/24,hh*size/24,c)
 if k=='search':e('lens',3.5,3.5,12,12);p('handle',[(14,14),(20.5,20.5)],1.8)
 elif k.startswith('chevron'):
  pts={'chevron.up':[(5,15),(12,8),(19,15)],'chevron.down':[(7,10),(12,15),(17,10)],'chevron.left':[(15,6),(9,12),(15,18)],'chevron.right':[(9,6),(15,12),(9,18)]}[k];p('outline',pts,1.9)
 elif k=='plus':p('horizontal',[(5,12),(19,12)]);p('vertical',[(12,5),(12,19)])
 elif k=='xmark':p('a',[(7,7),(17,17)]);p('b',[(17,7),(7,17)])
 elif k=='sidebar':shape(b,k+'/outline',x+2,y+4,size-4,size-8,'#ffffff',3,c);p('divider',[(9,5),(9,19)])
 elif k=='folder':p('outline',[(3,7),(3,19),(21,19),(21,7),(12,7),(10,4),(3,4),(3,7)])
 elif k=='doc':p('outline',[(6,3),(14,3),(19,8),(19,21),(6,21),(6,3)]);p('fold',[(14,3),(14,8),(19,8)]);p('line1',[(9,12),(16,12)]);p('line2',[(9,16),(16,16)])
 elif k=='checklist':
  for yy in [6,12,18]:e('circle'+str(yy),3,yy-2,4,4);p('line'+str(yy),[(11,yy),(21,yy)])
 elif k=='people':
  e('head1',5,3,6,6);e('head2',14,5,5,5);p('body1',[(2,20),(3,15),(6,12),(10,12),(13,15),(14,20)]);p('body2',[(15,13),(18,13),(21,16),(22,20)])
 elif k=='up':p('stem',[(12,19),(12,5)],2);p('head',[(6,11),(12,5),(18,11)],2)
 elif k=='check':p('mark',[(5,12),(10,17),(20,6)],1.8)
 elif k=='filter':
  for xx,yy,ww in [(3,6,18),(6,12,12),(9,18,6)]:p('line'+str(yy),[(xx,yy),(xx+ww,yy)])
 elif k=='refresh':p('arc',[(18,7),(15,4),(9,4),(5,7),(3,12),(5,18),(10,21),(16,19),(20,15)]);p('head',[(18,2),(18,8),(12,8)])
 elif k=='link':p('chain1',[(10,7),(13,4),(18,4),(21,7),(21,10),(17,14)]);p('chain2',[(14,17),(11,20),(6,20),(3,17),(3,14),(7,10)]);p('center',[(8,16),(16,8)])
 elif k=='clock':e('circle',3,3,18,18);p('hands',[(12,6),(12,12),(16,14)])
 elif k=='terminal':shape(b,'terminal/outline',x+2,y+4,size-4,size-8,'#ffffff',3,c);p('chev',[(6,8),(10,12),(6,16)]);p('cursor',[(13,16),(18,16)])
 elif k=='ellipsis':
  for xx in [5,12,19]:ellipse(b,'dot',x+xx*size/24-1,y+size/2-1,2,2,c,True)
 elif k=='lock':shape(b,'lock/body',x+5,y+10,size-10,size-12,'#ffffff',2,c);p('shackle',[(8,10),(8,5),(10,3),(14,3),(16,5),(16,10)])
 elif k=='warning':p('triangle',[(12,3),(22,21),(2,21),(12,3)]);p('stem',[(12,8),(12,14)]);ellipse(b,'dot',x+size/2-1,y+size*17/24,2,2,c,True)
 elif k=='pencil':p('pen',[(4,17),(4,21),(8,21),(21,8),(17,4),(4,17)]);p('cap',[(15,6),(19,10)])
 elif k=='gear':e('ring',4,4,16,16);e('hub',9,9,6,6)
 elif k=='bell':p('bell',[(5,17),(7,14),(7,8),(9,5),(15,5),(17,8),(17,14),(19,17),(5,17)]);p('bottom',[(10,20),(14,20)])
 else:p('line',[(5,12),(19,12)])
def master(k,w,h,draw):
 if k not in masters:
  m=base('SymbolMaster',k,0,0,w,h);m.update(allowsOverrides=True,hasBackgroundColor=False,includeBackgroundColorInInstance=False,isFlowHome=False,symbolID=uid());draw(m);masters[k]=m
 return masters[k]
def instance(b,m,x,y,target=None):
 a=base('SymbolInstance',m['name'],x,y,m['frame']['width'],m['frame']['height']);a.update(symbolID=m['symbolID'],scale=1)
 put(b,a)
 if target:link(a,target)
 return a
def symbol_icon(b,k,x,y,size=20,c=INK,target=None):
 m=master('SF-style/'+k+'/'+c+'/'+str(size),size,size,lambda m:icon_raw(m,k,0,0,size,c));return instance(b,m,x,y,target)
def button(b,s,x,y,w=84,target=None,kind='plain',icon=None,disabled=False):
 h=28
 fill=('#b14b3c' if kind=='danger' else ACCENT) if kind in ['primary','danger'] and not disabled else '#f3f3f3' if kind in ['bordered','primary','danger'] else '#ffffff'
 fg='#ffffff' if kind in ['primary','danger'] and not disabled else '#b6b6b6' if disabled else '#5c5c5c'
 key='Control/'+kind+'/'+str(w)+'/'+str(disabled)+'/'+str(bool(icon))
 def draw(m):
  shape(m,'button/background',0,0,w,h,fill,7,LINE if kind=='bordered' else None,kind=='bordered',alpha=0 if kind=='plain' else 1)
  if icon:symbol_icon(m,icon,7,6,16,fg)
  a=label(m,'Label',27 if icon else 11,5,w-30 if icon else w-18,12,fg,h=20,weight='medium');overrides[key]=a['do_objectID']
 m=master(key,w,h,draw);a=instance(b,m,x,y,target if not disabled else None)
 v=obj('OverrideValue');v.update(overrideName=overrides[key]+'_stringValue',value=s);a['overrideValues']=[v];a['name']=s;return a
def chip(b,s,x,y,w,selected=False,target=None):
 shape(b,s,x,y,w,24,MINT if selected else '#f5f5f5',12)
 label(b,s,x+10,y+4,w-16,11,ACCENT if selected else '#767676',h=18)
 if target:hotspot(b,s+'热点',x,y,w,24,target)
def glass(b,n,x,y,w,h,r=None):
 return shape(b,n,x,y,w,h,'#ffffff',r or h/2,'#dedede',True,alpha=1 if n.startswith(('sheet/','menu/','native/')) else .94)
def avatar(b,kind,x,y,size=32):
 colors={'captain':('#f5e7f8','🧩'),'backend':('#eaf2e8','🧩'),'design':('#e9eef8','🎨'),'group':('#f2e9f7','🧩'),'machine':('#f5f8df','🚀')}
 bg,em=colors[kind]
 m=master('Avatar/'+kind+'/'+str(size),size,size,lambda m:(shape(m,'圆形淡色底',0,0,size,size,bg,size/2,'#e4e4e4'),label(m,em,4,2,size-4,size*.65,INK,h=size)))
 return instance(b,m,x,y)
def tabs(b,selected):
 glass(b,'workspace/连续玻璃Tabs',252,9,418,38,19)
 widths=[60,70,70,98,112];items=[('消息','群聊'),('人待办','人待办'),('AI待办','AI待办'),('文档/数据','文档数据'),('计划/进展','计划进展')]
 x=257
 for (t,target),ww in zip(items,widths):
  if selected==target:shape(b,'当前Tab',x,14,ww-2,28,'#dfe8e3',14)
  label(b,t,x+12,19,ww-15,12,INK,h=18,weight='semibold' if selected==target else 'regular');hotspot(b,'Tab/'+t,x,13,ww,30,target);x+=ww
 glass(b,'toolbar/search glass',1234,12,30,30,15);symbol_icon(b,'search',1241,19,16,INK,'搜索')
 shape(b,'workspace/底部分隔',238,54,1042,.6,LINE)
def legacy_shell(s,b,selected='群聊'):
 shape(b,'window/白色共用画布',0,0,W,H,'#ffffff',16)
 shape(b,'sidebar/系统材质',0,0,238,H,'#efeeeb',16)
 # Sidebar right edge remains straight against the common workspace.
 shape(b,'sidebar/右缘',220,0,18,H,'#efeeeb')
 for i,c in enumerate(['#ff5f57','#febc2e','#28c840']):ellipse(b,'window/traffic light',20+i*20,24,12,12,c,True)
 glass(b,'sidebar/toggle glass',193,14,30,30,15);symbol_icon(b,'sidebar',200,21,16)
 # A native small segmented selector and adjacent tools, exactly in the sidebar.
 shape(b,'sidebar/selector',14,65,142,26,'#e3e2df',6)
 contacts=s=='通讯录';shape(b,'sidebar/selection',16 if contacts else 86,67,68,22,ACCENT,5)
 label(b,'通讯录',28,71,61,12,'#ffffff' if contacts else '#4d4d4d',h=20,weight='medium');label(b,'消息',107,71,45,12,INK if contacts else '#ffffff',h=20,weight='medium')
 hotspot(b,'通讯录入口',16,65,68,26,'通讯录');hotspot(b,'消息入口',86,65,68,26,'群聊');symbol_icon(b,'plus',166,68,20,MUTED,'新建机组');symbol_icon(b,'filter',201,68,20,MUTED,'消息分类')
 if contacts:
  for i,(t,level,target) in enumerate([('Mac Studio',0,'群聊'),('PendingName',1,'群聊'),('PendingCrew',2,'机组详情'),('后端实现',3,'另一机组'),('前端设计',3,'群聊'),('产品讨论',1,'群聊'),('前端设计 ↗',2,'群聊'),('MacBook Pro',0,'机器切换')]):
   yy=115+i*39;symbol_icon(b,'chevron.down' if i<3 else 'chevron.right',16+level*12,yy+3,12,MUTED);label(b,t,34+level*12,yy,176-level*12,13,h=24);hotspot(b,'通讯录/'+t,16,yy,205,30,target)
 else:
  crewrows=[('京都' if s=='已创建机组' else 'PendingCrew','把团队留在你身边','12:42','机组详情'),('后端实现','重连验证记录已更新','12:18','另一机组'),('前端设计','正在细化原生控件','昨天','群聊'),('PendingName','今天从这里开始','周五','另一机组')]
  for i,(t,sub,tm,target) in enumerate(crewrows):
   yy=107+i*70
   if i==0:shape(b,'sidebar/选中机组',12,yy,214,60,MINT,12)
   label(b,t,26,yy+9,139,13,INK,h=22,weight='semibold' if i==0 else 'medium');label(b,tm,177,yy+11,40,10,MUTED,h=18)
   label(b,sub,26,yy+32,182,11,MUTED,h=20);hotspot(b,'sidebar/机组 '+t,14,yy,210,60,target)
  label(b,'已隐藏的机组',26,413,170,11,'#999999');symbol_icon(b,'chevron.right',205,414,12,'#999999')
 # Footer is a small real repeated Symbol assembly, not a card.
 avatar(b,'machine',18,766,30);label(b,'Mac Studio',58,773,118,13,INK,h=22,weight='semibold');symbol_icon(b,'chevron.up',172,774,19,INK);ellipse(b,'machine/online',202,781,7,7,'#34c759',True);hotspot(b,'机器菜单入口',12,756,210,54,'机器菜单')
 if s in ['新建机组','填写机组','空白']:
  glass(b,'toolbar/search glass',1234,12,30,30,15);symbol_icon(b,'search',1241,19,16,INK,'搜索');shape(b,'pending/header divider',238,54,1042,.6,LINE);return
 tabs(b,selected)
 shape(b,'会话列/细分隔',451,54,.6,766,'#eeeeee');label(b,'会话',252,70,150,11,'#909090',h=20);symbol_icon(b,'plus',405,66,20,'#8d8d8d','新建机组')
 conversations=[('大厅','机长：布局和记录都保留在这里','group','群聊'),('与你沟通','关于这一次设计的决定','backend','单聊'),('后端 · 重连验证','3 条新消息','backend','另一机组'),('设计 · 原生界面','文档已更新','design','单聊')]
 for i,(t,sub,av,target) in enumerate(conversations):
  yy=104+i*69;sel=(s=='单聊' and i==1)or(s=='另一机组' and i==2)or(s not in ['单聊','另一机组']and i==0)
  if sel:shape(b,'会话/选中行',246,yy,180,61,'#f3f3f3',7)
  avatar(b,av,258,yy+12,34);label(b,t,302,yy+11,112,13,INK,h=21,weight='semibold' if sel else 'medium');label(b,sub,302,yy+34,112,10,MUTED,h=18);hotspot(b,'会话/'+t,246,yy,180,61,target)
 # A light chevron at the shared edge, no button plate.
 symbol_icon(b,'chevron.left',432,423,16,'#9c9c9c','列表折叠')
def composer(b,placeholder='发群消息',enabled=False,metadata=False):
 if metadata:
  labels=[('目录','crewground' if enabled else '未指定'),('机组名称','京都' if enabled else '未指定'),('coding agent','Codex'),('机器','Mac Studio'),('父机组','PendingCrew')]
  for i,(t,v) in enumerate(labels):
   x=256+i*198;y=666
   label(b,t,x,y,190,10,MUTED,h=18);shape(b,'metadata/'+t,x,y+20,184,28,'#f8f8f8',6,'#e6e6e6');label(b,v,x+9,y+25,151,12,'#aaaaaa' if v=='未指定' else INK,h=19)
   if i in [2,3,4]:symbol_icon(b,'chevron.down',x+164,y+29,12,MUTED)
   elif i==0:symbol_icon(b,'folder',x+163,y+27,15,MUTED)
   hotspot(b,'编辑/'+t,x,y+20,184,28,'填写机组')
 else:label(b,'草稿仅本机保存',510,725,300,10,'#9b9b9b',h=18);symbol_icon(b,'doc',1197,723,14,MUTED);label(b,'草稿',1216,725,52,10,MUTED,h=18)
 glass(b,'composer/checklist',254 if metadata else 509,755,38,38,19);symbol_icon(b,'checklist',263 if metadata else 518,764,20,'#77776e','人待办')
 glass(b,'composer/input pill',303 if metadata else 559,752,960 if metadata else 704,44,22);label(b,placeholder,320 if metadata else 576,765,855 if metadata else 620,14,'#aaaaa6' if not enabled else INK,h=24)
 ellipse(b,'composer/发送底',1224,760,29,29,ACCENT if enabled else '#f4f5f3',True);symbol_icon(b,'up',1230,766,17,'#ffffff' if enabled else '#d5d8d2','已创建机组' if metadata else '群聊')
 if metadata and not enabled:hotspot(b,'首条消息输入',319,755,880,40,'填写机组')
def chatbar(b):
 label(b,'大厅',503,68,90,11,'#8e8e8e',h=20)
 control_start=len(b['layers'])
 button(b,'消息分类',832,62,100,'消息分类',icon='folder');symbol_icon(b,'chevron.down',916,72,10,MUTED)
 button(b,'人类沟通',942,62,100,'群聊');symbol_icon(b,'chevron.down',1029,71,10,MUTED)
 chip(b,'仅 @你',1051,65,73,False,'群聊');symbol_icon(b,'people',1135,68,17,'#4f4f4f','成员');label(b,'成员',1158,70,55,11,'#4f4f4f',h=18);hotspot(b,'成员入口',1131,62,84,28,'成员')
 shape(b,'聊天/工具栏分隔',486,98,794,.6,LINE)
 for layer in b['layers'][control_start:-1]:layer['userInfo']={'anchoredToolbar':True}
def bubble(b,s,y,own=False,name='机长',av='captain',height=62,sub=None):
 if own:
  xx=807;ww=430;shape(b,'消息/你',xx,y,ww,height,MINT,18);a=label(b,s,xx+16,y+12,ww-32,14,INK,h=height-16);a['userInfo']={'anchoredRight':True};return
 avatar(b,av,507,y,32);label(b,name,549,y-1,500,11,MUTED,h=18);label(b,s,549,y+22,674,14,INK,h=height)
 if sub:label(b,sub,549,y+height+21,640,11,MUTED,h=30)
def chat(b,s):
 chatbar(b);label(b,'今天 12:42',833,118,200,10,'#ababab',h=18)
 bubble(b,'我想让它像一个可靠的私人团队。\n消息简单一点，其他事情都能顺手找到。',154,True,height=68)
 bubble(b,'我们沿着原来的聊天来做。\n消息、待办、资料与进展，还是你熟悉的五个入口。',249,height=61)
 bubble(b,'我把这次要确认的内容整理好了：\n• 顶部保留连续胶囊，搜索只留一个入口\n• 新机组直接开始聊天，细节放在输入框上方\n• 每个群保留各自的消息、成员和工作',355,name='设计',av='design',height=114)
 chip(b,'文档 · 原生界面说明',550,493,164,False,'文档数据');symbol_icon(b,'link',724,498,14,MUTED)
 bubble(b,'@你  还有两项决定需要你看一下。',556,name='后端',av='backend',height=30)
 chip(b,'人待办  ·  确认原型范围',549,611,186,True,'人待办')
 composer(b)
def todo(b,s):
 label(b,'当前群',507,69,82,12,INK,h=20);symbol_icon(b,'chevron.down',566,73,11,MUTED)
 button(b,'新建人待办',1139,63,124,'编辑待办',kind='bordered',icon='plus')
 shape(b,'todo/分隔',486,100,794,.6,LINE)
 chip(b,'所有待办  5',507,115,104,True);chip(b,'设计确认  3',620,115,104,False,'待办分组');chip(b,'首版验收  2',733,115,104,False,'待办分组');chip(b,'等待外部  0',846,115,104,False,'筛选为空')
 symbol_icon(b,'filter',1206,118,18,MUTED,'待办分组');symbol_icon(b,'refresh',1242,118,18,MUTED,'人待办')
 label(b,'待你处理',508,160,500,11,MUTED,h=20);label(b,'3 项',1228,160,40,11,MUTED,h=20)
 rows=[('确认原生界面的布局与交互','你  ·  设计确认  ·  今天 12:38','high'),('决定新机组的默认工作目录','你  ·  首版验收  ·  今天 11:52','normal'),('检查暂停后的恢复提示','你  ·  首版验收  ·  昨天','normal')]
 for i,(title,detail,_) in enumerate(rows):
  yy=190 if i==0 else 487+(i-1)*76
  if i==0:shape(b,'todo/当前事项',501,yy,766,276,'#fdfdfc',9,'#e5e5e3')
  ellipse(b,'todo/check',515,yy+14,15,15,'#9a8b62',False,1.2);label(b,title,541,yy+10,665,13,INK,h=23,weight='medium');label(b,detail,541,yy+36,680,11,MUTED,h=20);symbol_icon(b,'ellipsis',1235,yy+11,18,MUTED,'编辑待办')
  hotspot(b,'待办/'+title,514,yy+7,700,48,'编辑待办')
  if i>0:shape(b,'todo/行分隔',513,yy+67,741,.5,LINE)
 label(b,'原始记录',542,262,650,11,MUTED,h=18)
 label(b,'保留 SwiftUI 的原有形态。先看聊天、新建机组、待办与文档，\n再把一致的控件和内容密度推广到完整状态。',542,287,660,13,INK,h=48)
 chip(b,'关联文档 · 原生界面说明',542,347,191,False,'文档数据');chip(b,'关联进展 · 前端精修',742,347,167,False,'计划进展')
 label(b,'你的处理',542,391,100,11,MUTED,h=20);button(b,'负责人、状态与标签',630,384,175,'编辑待办',kind='bordered');button(b,'标记完成',1102,414,111,'待办完成',kind='primary')
 label(b,'已完成  2',507,660,600,11,MUTED,h=18);symbol_icon(b,'chevron.down',1239,663,12,MUTED)
 for title,sub,yy in [('保留群聊与单聊的混合会话列表','你  ·  设计确认  ·  今天 10:12',689),('统一机器菜单的中性色彩','你  ·  设计确认  ·  昨天',738)]:
  ellipse(b,'todo/completed circle',515,yy+4,15,15,'#9a8b62',False);symbol_icon(b,'check',517,yy+6,11,'#9a8b62');label(b,title,541,yy,670,13,'#999999',h=22);label(b,sub,541,yy+24,670,11,'#aaaaaa',h=18)
 label(b,'共 5 项  ·  待处理 3 项  ·  已完成 2 项',507,795,650,10,'#a2a2a2',h=18)

def document(b,s):
 label(b,'本群引用',507,69,90,12,INK,h=20);symbol_icon(b,'chevron.down',572,73,11,MUTED);label(b,'文档 / 知识切片',610,69,180,12,INK,h=20);symbol_icon(b,'chevron.down',718,73,11,MUTED,'页面预览');hotspot(b,'资料类型/Agent页面',606,63,130,28,'页面预览')
 symbol_icon(b,'refresh',1209,68,18,MUTED,'读取失败');symbol_icon(b,'ellipsis',1242,67,19,MUTED,'片段编辑');shape(b,'资料/分隔',486,100,794,.6,LINE)
 shape(b,'资料列表/分隔',709,100,.6,680,LINE)
 label(b,'文档',506,117,170,11,MUTED,h=18)
 docs=[('原生界面说明','设计 · 3 个片段','doc'),('重连验证记录','后端 · 6 个片段','doc'),('产品用语','机长 · 2 个片段','doc')]
 for i,(title,sub,ico) in enumerate(docs):
  yy=148+i*70
  if i==0:shape(b,'资料/选中行',498,yy,201,62,'#f4f4f4',7)
  symbol_icon(b,ico,510,yy+12,17,'#7f8c83');label(b,title,536,yy+10,157,13,INK,h=22,weight='medium');label(b,sub,536,yy+35,155,10,MUTED,h=18);hotspot(b,'资料/'+title,498,yy,201,62,'片段编辑')
 label(b,'仅显示本群已关联的文档',507,398,180,10,'#aaaaaa',h=38)
 x=733
 label(b,'原生界面说明',x,121,435,20,INK,h=33,weight='semibold');symbol_icon(b,'pencil',1237,125,18,MUTED,'片段编辑')
 label(b,'设计  ·  今天 12:35 更新  ·  版本 '+('5' if s=='片段保存' else '4'),x,162,490,11,MUTED,h=20);chip(b,'设计确认',x,194,80,False,'关联示例');chip(b,'本群引用',x+89,194,80,False,'关联示例')
 shape(b,'文档/细分隔',x,237,523,.5,LINE)
 label(b,'01  保留熟悉的聊天',x,259,500,14,INK,h=24,weight='semibold');label(b,'这个工作区首先是一段可以继续的对话。\n消息、待办、资料与进展共享同一组顶部入口，\n每个群保留自己的成员与上下文。',x,299,500,13,'#4e4e4e',h=81)
 label(b,'02  使用原生的控件与尺度',x,405,500,14,INK,h=24,weight='semibold');label(b,'按钮、列表和弹出菜单遵循 macOS 的节奏。\n长内容留给白色画布，材质只用于顶部导航、\n搜索入口和底部输入区域。',x,445,500,13,'#4e4e4e',h=81)
 label(b,'关联示例',x,554,500,11,MUTED,h=18)
 for i,(t,target) in enumerate([('群聊 · 顶部导航与输入框','群聊'),('新建机组 · 首条消息','新建机组')]):
  yy=587+i*38;symbol_icon(b,'link',x,yy,15,MUTED);label(b,t,x+26,yy-1,455,12,'#606c64',h=23);symbol_icon(b,'chevron.right',1237,yy+1,12,MUTED,target);hotspot(b,'文档关联/'+t,x,yy,500,30,target)
 button(b,'编辑片段',x,706,99,'片段编辑',kind='bordered',icon='pencil');button(b,'页面预览',x+111,706,101,'生成页面',kind='bordered',icon='doc')
 label(b,'3 个片段  ·  2 个关联示例  ·  已保存',x,782,520,10,'#9c9c9c',h=18)

# Each retained state has a deliberate content/interaction treatment.
# Sheets and popovers are native editable layer compositions with back flows.
def panel(b,title,subtitle='',x=667,y=153,w=548,h=510):
 # A slight native sheet shadow and hairline keep the white content distinct.
 glass(b,'sheet/'+title,x,y,w,h,14);label(b,title,x+22,y+20,w-74,17,INK,h=29,weight='semibold')
 if subtitle:label(b,subtitle,x+22,y+55,w-44,11,MUTED,h=42)
 symbol_icon(b,'xmark',x+w-40,y+24,16,MUTED,'back')
 return x+22,y+107,w-44,y+h-51

def field(b,title,value,x,y,w=450,kind='text',target=None):
 label(b,title,x,y,w,11,MUTED,h=18);shape(b,'field/'+title,x,y+23,w,30,'#ffffff',6,'#d8d8d8')
 label(b,value,x+10,y+30,w-28,13,INK,h=23)
 if kind=='menu':symbol_icon(b,'chevron.down',x+w-23,y+32,12,MUTED)
 if target:hotspot(b,'field/'+title,x,y+23,w,30,target)

def notice(b,title,detail,x=519,y=115,w=720,warning=False):
 shape(b,'notice/'+title,x,y,w,67,'#fbf5e7' if warning else '#f1f5f2',8)
 symbol_icon(b,'warning' if warning else 'check',x+12,y+15,17,'#96722e' if warning else '#668b76')
 label(b,title,x+40,y+11,w-55,12,INK,h=21,weight='medium');label(b,detail,x+40,y+37,w-55,11,MUTED,h=25)

def progress(b,s):
 label(b,'机组全部',507,69,100,12,INK,h=21);symbol_icon(b,'chevron.down',578,73,11,MUTED)
 button(b,'关联计划',1144,63,119,'步骤展开',kind='bordered',icon='link');shape(b,'progress/separator',486,100,794,.6,LINE)
 chip(b,'进行中  2',508,116,93,True);chip(b,'等待验收  1',610,116,108,False,'进展折叠');label(b,'更新于 今天 12:40',1091,122,172,10,MUTED,h=18)
 collapsed=s=='进展折叠'
 symbol_icon(b,'chevron.right' if collapsed else 'chevron.down',511,174,15,MUTED,'计划进展' if collapsed else '进展折叠')
 label(b,'原生界面精修',538,169,610,16,INK,h=28,weight='semibold');chip(b,'进行中',1154,170,82,True)
 label(b,'设计  ·  关联待办 H-21  ·  更新 12:40',538,203,630,11,MUTED,h=20)
 if collapsed:
  label(b,'3 个步骤  ·  1 个已完成  ·  下一步：与你确认原型',538,239,650,12,'#686868',h=24)
  shape(b,'progress/divider',509,289,740,.6,LINE)
  symbol_icon(b,'chevron.right',511,320,15,MUTED,'步骤展开');label(b,'重连与恢复验收',538,316,600,15,INK,h=27,weight='medium');label(b,'后端  ·  等待验收  ·  2 份证据',538,352,600,11,MUTED,h=22)
  return
 label(b,'目标',538,253,68,11,MUTED,h=19);label(b,'沿着当前 SwiftUI，完成可继续讨论的原生设计。',604,250,590,13,INK,h=29)
 label(b,'下一步',538,290,68,11,MUTED,h=19);label(b,'确认控件、内容密度和主要流程，再进入产品集成。',604,287,590,13,INK,h=29)
 shape(b,'progress/rule',538,338,698,.6,LINE)
 rows=[('01','读取实际界面与源码','已完成','已对照 common-top 实图与原生控件'),('02','细化 54 个设计状态','进行中','可编辑原生图层、复用组件、真实返回连接'),('03','确认设计后再集成','未开始','等待人类确认，不自动继续实现')]
 for i,(num,t,status,detail) in enumerate(rows):
  yy=369+i*111 if s!='步骤展开' else 369+i*136
  ellipse(b,'step/'+num,539,yy,23,23,'#8b9e91' if i==0 else '#aaaaaa',False,1)
  if i==0:symbol_icon(b,'check',545,yy+6,12,'#6b8e78')
  else:label(b,num,545,yy+5,21,9,MUTED,h=18)
  label(b,t,578,yy-1,570,13,INK,h=23,weight='medium');label(b,status,1166,yy+1,80,11,MUTED,h=20)
  label(b,detail,578,yy+30,650,11,MUTED,h=26)
  hotspot(b,'step/'+t,537,yy,700,65,'计划进展' if s=='步骤展开' else '步骤展开')
  if s=='步骤展开' and i==0:chip(b,'文档 · 原生界面说明',579,yy+70,170,False,'文档数据');chip(b,'待办 · H-21',759,yy+70,105,False,'人待办')
 label(b,'记录状态与证据分开；设计完成不代表产品已验收。',509,791,740,10,'#a0a0a0',h=18)

def completed(b):
 label(b,'当前群',507,69,120,12,INK,h=20);button(b,'新建人待办',1139,63,124,'编辑待办',kind='bordered',icon='plus')
 notice(b,'已标记完成','确认原生界面的布局与交互 · 负责人：你',y=115)
 button(b,'撤销完成',1132,135,110,'人待办',kind='plain')
 label(b,'待处理  2',508,211,720,11,MUTED,h=20)
 for i,t in enumerate(['决定新机组的默认工作目录','检查暂停后的恢复提示']):
  yy=250+i*78;ellipse(b,'todo/open',515,yy+3,15,15,'#9a8b62',False,1.2);label(b,t,541,yy,650,13,h=24,weight='medium');label(b,'你  ·  首版验收',541,yy+29,650,11,MUTED,h=20);hotspot(b,t,508,yy-4,730,62,'编辑待办');shape(b,'row/divider',513,yy+64,728,.5,LINE)
 label(b,'已完成  3',508,437,720,11,MUTED,h=20)
 for i,t in enumerate(['确认原生界面的布局与交互','保留群聊与单聊的混合会话列表','统一机器菜单的中性色彩']):
  yy=478+i*77;ellipse(b,'todo/done',515,yy+3,15,15,'#9a8b62',False,1.2);symbol_icon(b,'check',517,yy+5,11,'#9a8b62');label(b,t,541,yy,650,13,'#8f8f8f',h=24);label(b,'你  ·  设计确认',541,yy+29,650,11,'#aaaaaa',h=20)
 label(b,'共 5 项  ·  待处理 2 项  ·  已完成 3 项',507,790,650,10,MUTED,h=18)

def human_editor(b):
 x,y,w,foot=panel(b,'负责人、状态与标签','确认原生界面的布局与交互',h=491)
 field(b,'负责人','你',x,y,w,'menu','待办分组');field(b,'状态','待处理',x,y+76,w,'menu','待办完成');field(b,'标签','设计确认',x,y+152,w,'menu','待办分组')
 label(b,'修改不改变原始记录和关联的文档。',x,foot-42,w,11,MUTED,h=22);button(b,'取消',x+279,foot,84,'back',kind='bordered');button(b,'保存',x+375,foot,84,'人待办',kind='primary')

def ai_todo(b,s):
 label(b,'当前群',507,69,120,12,INK,h=20);button(b,'新建 AI 待办',1127,63,136,'AI待办详情',kind='bordered',icon='plus');shape(b,'AI/rule',486,100,794,.6,LINE)
 chip(b,'所有待办  3',507,115,108,True);chip(b,'首版验收  2',626,115,108,False,'待办分组');chip(b,'设计确认  1',745,115,108,False,'待办分组')
 for i,(t,sub,status) in enumerate([('整理重连与恢复的验证记录','后端  ·  首版验收  ·  关联 P-12','进行中'),('对照 SwiftUI 完成原生设计','设计  ·  设计确认  ·  关联 P-14','等待验收'),('保存暂停前的工作上下文','机长  ·  首版验收','已完成')]):
  yy=180+i*102;ellipse(b,'AI/status',515,yy+5,15,15,'#527462',False,1.2);label(b,t,542,yy,620,13,h=24,weight='medium');label(b,sub,542,yy+31,630,11,MUTED,h=22);label(b,status,1176,yy+2,80,11,MUTED,h=20);shape(b,'AI/divider',514,yy+81,729,.5,LINE);hotspot(b,t,508,yy-3,740,77,'AI待办详情')
 if s=='AI待办详情':
  x,y,w,foot=panel(b,'整理重连与恢复的验证记录','AI-20  ·  后端  ·  进行中',h=509)
  label(b,'原始需求',x,y,w,11,MUTED,h=20);label(b,'保留每次连接变化的证据，核对未发送内容不会\n被自动重发，恢复操作只由人显式触发。',x,y+31,w,13,h=66)
  field(b,'负责人','后端',x,y+117,w,'menu');field(b,'关联计划','P-12 · 重连与恢复验收',x,y+194,w,'menu','步骤展开')
  button(b,'查看文档',x,foot,116,'文档数据',kind='bordered',icon='doc');button(b,'返回 AI 待办',x+303,foot,156,'AI待办',kind='primary')

def doc_overlay(b,s):
 if s=='片段保存':notice(b,'片段已保存','版本 5 · 原文与关联示例保持同一份记录',x=738,y=679,w=511);return
 if s=='页面预览':
  # Collection=web: a separate native page surface, never mixed into document rows.
  shape(b,'web/清除文档内容',486,55,794,765,'#ffffff');label(b,'本群引用',507,69,100,12,h=22);symbol_icon(b,'chevron.down',572,73,11,MUTED);label(b,'Agent 自由页面',610,69,180,12,h=22);symbol_icon(b,'chevron.down',718,73,11,MUTED,'文档数据');shape(b,'web/rule',486,100,794,.6,LINE)
  label(b,'团队设计手册',518,127,650,18,h=31,weight='semibold');label(b,'原生页面预览  ·  未发布  ·  不自动运行 Agent',518,167,650,11,MUTED,h=21)
  shape(b,'web/previewframe',516,214,730,482,'#ffffff',12,'#e4e4e4')
  label(b,'PendingCrew',550,249,610,24,h=40,weight='semibold');label(b,'一个可以继续的私人团队',550,306,610,18,h=34)
  label(b,'消息保持自然。\n需要你决定的事留在待办，需要反复使用的内容留在资料。',550,369,620,14,'#5d5d5d',h=65)
  for i,(t,d) in enumerate([('01  沿着原来的对话','保留消息与上下文，不为切换视图重开工作。'),('02  让证据跟着进展','步骤、检查项、资料与源待办可以相互返回。')]):label(b,t,550,474+i*86,620,15,h=26,weight='medium');label(b,d,550,509+i*86,620,12,MUTED,h=26)
  button(b,'返回文档 / 知识切片',517,726,195,'文档数据',kind='bordered');return
 if s=='片段编辑':
  x,y,w,foot=panel(b,'编辑知识切片','文档：原生界面说明  ·  片段 02  ·  版本 4',h=556)
  field(b,'标题','使用原生的控件与尺度',x,y,w)
  label(b,'内容',x,y+79,w,11,MUTED,h=20);shape(b,'editor/content',x,y+104,w,132,'#ffffff',7,'#9ab8aa');label(b,'按钮、列表和弹出菜单遵循 macOS 的节奏。\n长内容留给白色画布，材质只用于顶部导航、\n搜索入口和底部输入区域。',x+12,y+117,w-24,13,h=104)
  label(b,'当前可编辑 · 修改将生成新版本',x,y+253,w,11,MUTED,h=22)
  button(b,'权限',x,foot,80,'只读权限',icon='lock');button(b,'提交记录',x+87,foot,111,'提交结果未知',icon='clock');button(b,'取消',x+279,foot,84,'back',kind='bordered');button(b,'保存',x+375,foot,84,'片段保存',kind='primary');return
 if s=='关联示例':
  x,y,w,foot=panel(b,'关联示例','为当前片段选择已有的设计状态。',h=394)
  for i,t in enumerate(['群聊 · 顶部导航与输入框','新建机组 · 首条消息']):
   yy=y+i*56;shape(b,'checkbox',x,yy+3,14,14,'#6c8275',3);symbol_icon(b,'check',x+2,yy+5,10,'#ffffff');label(b,t,x+27,yy,w-30,13,h=25);hotspot(b,'关联/'+t,x,yy,w,38,'文档数据')
  label(b,'引用同一份片段，不复制出另一份正文。',x,y+131,w,11,MUTED,h=26);button(b,'取消',x+279,foot,84,'back',kind='bordered');button(b,'保存关联',x+375,foot,84,'片段保存',kind='primary');return
 if s=='生成页面':
  x,y,w,foot=panel(b,'打开页面预览','团队设计手册',h=399)
  symbol_icon(b,'doc',x,y,25,'#7b8c80');label(b,'来源：原生界面说明 · 版本 5',x+39,y+2,w-43,13,h=27)
  label(b,'仅预览已经保存的页面内容。\n不会公开发布，不会启动或恢复 Agent。',x,y+65,w,13,'#646464',h=68)
  button(b,'返回文档',x+235,foot,112,'文档数据',kind='bordered');button(b,'打开预览',x+359,foot,100,'页面预览',kind='primary')

def members(b,s):
 x,y,w,foot=panel(b,'本群成员' if s=='成员' else '后端','4 人  ·  大厅' if s=='成员' else 'PendingCrew · Codex 会话',x=948,y=97,w=314,h=444)
 if s=='成员':
  for i,(n,d,av) in enumerate([('你','人类','machine'),('机长','Codex · 等待安排','captain'),('后端','Codex · 工作中','backend'),('设计','Codex · 等待消息','design')]):
   yy=y+i*65;avatar(b,av,x,yy,30);label(b,n,x+43,yy-1,w-45,13,h=23,weight='medium');label(b,d,x+43,yy+23,w-45,11,MUTED,h=20);hotspot(b,'member/'+n,x,yy,w,52,'成员详情')
 else:
  avatar(b,'backend',x,y,38);label(b,'后端',x+51,y+2,w-53,14,h=24,weight='semibold');label(b,'工作中',x+51,y+26,w-53,11,MUTED,h=20)
  label(b,'正在整理重连验证记录。\n所属机组：PendingCrew\n会话上下文已保留。',x,y+74,w,12,'#626262',h=102)
  button(b,'发消息',x,foot-37,w,'单聊',kind='bordered');button(b,'会话控制',x,foot,w,'Codex控制',kind='bordered')

def search_panel(b,s):
 x,y,w,foot=panel(b,'搜索','',x=825,y=54,w=438,h=415)
 shape(b,'search/scope',x,y-43,w,28,'#f0f0f0',7);shape(b,'search/selected',x+(w/2 if s=='全局搜索' else 0),y-42,w/2,26,'#ffffff',6,'#e5e5e5')
 label(b,'当前群',x+52,y-36,w/2,12,h=20);label(b,'全局 · 当前电脑',x+w/2+33,y-36,w/2-35,12,h=20);hotspot(b,'search/current',x,y-43,w/2,28,'搜索结果');hotspot(b,'search/global',x+w/2,y-43,w/2,28,'全局搜索')
 shape(b,'search/focused field',x,y+3,w,31,'#ffffff',6,'#8daea0');symbol_icon(b,'search',x+9,y+11,15,MUTED);label(b,'原生界面',x+34,y+10,w-40,13,h=22)
 for i,(title,sub,target) in enumerate([('原生界面说明','文档 · PendingCrew · 今天12:35','文档数据'),('确认原生界面的布局与交互','人待办 · 大厅 · 今天12:38','人待办'),('对照 SwiftUI 完成原生设计','AI待办 · 设计 · 进行中','AI待办')]):
  yy=y+57+i*60;symbol_icon(b,'doc' if i==0 else 'checklist',x+2,yy+3,16,MUTED);label(b,title,x+29,yy,w-34,12,h=22);label(b,sub,x+29,yy+24,w-34,10,MUTED,h=18);hotspot(b,'search/result '+title,x,yy,w,53,target)

def menu_panel(b,title,items,x=224,y=125,w=277):
 h=44+len(items)*35;glass(b,'menu/'+title,x,y,w,h,9);label(b,title,x+12,y+10,w-24,11,MUTED,h=20)
 for i,(t,k,target) in enumerate(items):
  yy=y+37+i*35
  if k:symbol_icon(b,k,x+12,yy+4,16,'#656565')
  label(b,t,x+39,yy+3,w-50,12,h=23);hotspot(b,'menu/'+t,x+6,yy,w-12,31,target)

def settings(b,s):
 x,y,w,foot=panel(b,'设置','机器、连接与通知在同一处管理。',x=631,y=132,w=604,h=565)
 for i,(t,target) in enumerate([('机器与连接','连接设置'),('通知','通知设置')]):chip(b,t,x+i*139,y-28,128,(s=='通知设置' and i==1)or(s!='通知设置'and i==0),target)
 if s=='通知设置':
  for i,(t,sub,on) in enumerate([('提及我时通知','仅与我有关的消息',True),('等待我决定的事项','待办与人工确认',True),('普通消息','其他群内消息',False),('安静时间','22:00 — 08:00',True)]):
   yy=y+27+i*75;label(b,t,x,yy,w-65,13,h=23,weight='medium');label(b,sub,x,yy+27,w-65,11,MUTED,h=21);shape(b,'toggle/'+t,x+w-41,yy+8,32,19,'#6a8a76' if on else '#dedede',10);ellipse(b,'toggle/thumb',x+w-(25 if on else 39),yy+10,15,15,'#ffffff',True);hotspot(b,'toggle/'+t,x+w-44,yy,44,35,'通知设置');shape(b,'setting/divider',x,yy+64,w,.5,LINE)
 else:
  for i,(name,sub,av,status) in enumerate([('Mac Studio','本机 · 当前使用','machine','在线'),('MacBook Pro','远端电脑 · 连接未建立','backend','离线')]):
   yy=y+30+i*92;avatar(b,av,x,yy,34);label(b,name,x+48,yy,w-150,14,h=23,weight='semibold');label(b,sub,x+48,yy+29,w-150,11,MUTED,h=22);label(b,status,x+w-54,yy+9,54,11,MUTED,h=22);hotspot(b,'machine/'+name,x,yy,w,71,'群聊' if i==0 else '机器切换');shape(b,'setting/divider',x,yy+77,w,.5,LINE)
  label(b,'连接状态',x,y+232,w,11,MUTED,h=20);label(b,'在线 · 可接收消息',x,y+263,w,13,h=25);button(b,'添加机器',x,foot-37,122,'机器切换',kind='bordered',icon='plus')
 button(b,'完成',x+w-85,foot,85,'群聊',kind='primary')

def controls(b,s):
 x,y,w,foot=panel(b,'Codex 会话','后端 · PendingCrew',x=655,y=142,w=567,h=538)
 for i,(t,v) in enumerate([('状态','已暂停 · 上下文保留'),('当前工作','整理重连与恢复的验证记录'),('模型','GPT-6 · High'),('工作目录','PendingCrew')]):
  yy=y+i*49;label(b,t,x,yy,98,11,MUTED,h=20);label(b,v,x+103,yy,w-105,12,h=24);shape(b,'session/divider',x,yy+34,w,.5,LINE)
 label(b,'重新打开不会自动继续。核对上下文后，再手动恢复。',x,y+216,w,11,MUTED,h=35)
 button(b,'模型选项',x,foot,112,'模型选择',kind='bordered');button(b,'手动恢复',x+w-118,foot,118,'恢复确认',kind='primary')
 if s=='模型选择':menu_panel(b,'选择模型',[('GPT-6 · High','check','Codex控制'),('GPT-6 · Medium',None,'Codex控制'),('GPT-6.1 · High',None,'Codex控制')],x=739,y=438,w=313)

def confirmation(b,s):
 data={
 '恢复确认':('手动恢复这个会话？','将使用已保留的上下文继续。请先核对机组与未完成工作。','确认恢复','已恢复','Codex控制'),
 '已恢复':('会话已恢复','由你显式恢复 · 工作中。你可以继续沟通或叫停当前工作。','叫停工','停工确认','群聊'),
 '停工确认':('叫停当前工作？','请求会话在安全点停止，并保存当前进度与上下文。','确认叫停工','已停工','已恢复'),
 '已停工':('当前工作已停止','进度与上下文已保留。继续需要再次手动恢复。','退出应用','退出询问','Codex控制'),
 '安全暂停':('安全暂停完成','恢复点已保留。下次打开不会自动执行。','重新打开','重新打开','空白'),
 '关闭等待窗':('等待窗口已关闭','停工请求仍保留，应用保持打开，尚未退出。','查看停工状态','已停工','群聊'),
 '强制关闭':('强制关闭 PendingCrew？','当前工作可能尚未保存。强制关闭会丢失本次未保存的上下文。','强制关闭','空白','等待停工')}
 title,detail,primary,target,cancel=data[s];x,y,w,foot=panel(b,title,detail,x=713,y=242,w=469,h=294)
 symbol_icon(b,'warning' if s=='强制关闭' else 'clock' if s=='停工确认' else 'check',x,y-8,27,'#98723c' if s=='强制关闭' else '#6e8b77')
 label(b,'当前会话：后端 · 上下文已保留',x+42,y-2,w-48,11,MUTED,h=42)
 button(b,'返回' if s in ['已停工','已恢复','安全暂停','关闭等待窗'] else '取消',x+157,foot,84,cancel,kind='bordered');button(b,primary,x+253,foot,150,target,kind='danger' if s=='强制关闭' else 'primary')

def quit_panel(b,s):
 if s=='退出询问':
  x,y,w,foot=panel(b,'退出 PendingCrew？','当前有 1 个会话仍在工作。',x=673,y=189,w=539,h=435)
  for i,(t,d,target) in enumerate([('直接退出','立即关闭应用。','空白'),('安全暂停','保存上下文后退出。','安全暂停'),('叫停工并退出','等待安全停工，再关闭应用。','等待停工')]):
   yy=y-8+i*83;symbol_icon(b,'clock' if i==2 else 'lock' if i==1 else 'xmark',x+4,yy+7,19,MUTED);label(b,t,x+41,yy,w-60,13,h=23,weight='medium');label(b,d,x+41,yy+29,w-60,11,MUTED,h=23);symbol_icon(b,'chevron.right',x+w-24,yy+15,13,MUTED);hotspot(b,'退出/'+t,x,yy,w,69,target)
  button(b,'取消',x+w-88,foot,88,'群聊',kind='bordered')
 else:
  x,y,w,foot=panel(b,'正在等待安全停工','后端正在整理本次进度。',x=713,y=235,w=469,h=326)
  symbol_icon(b,'clock',x,y,30,MUTED);label(b,'你可以关闭这个等待窗口。\n停工请求会保留，应用暂不退出。',x+47,y,w-51,12,'#666666',h=69)
  button(b,'强制关闭…',x,foot,112,'强制关闭',kind='bordered');button(b,'关闭等待窗口',x+251,foot,152,'关闭等待窗',kind='primary')

def boundaries(b,s):
 if s=='只读权限':
  x,y,w,foot=panel(b,'当前文档为只读','原生界面说明 · 版本 4',h=391)
  symbol_icon(b,'lock',x,y,28,MUTED);label(b,'你可以阅读与引用，不能修改原文。\n编辑需要文档所有者授予写入权限。',x+45,y,w-49,13,'#626262',h=77)
  button(b,'保存',x,foot,92,kind='primary',disabled=True);button(b,'返回文档',x+w-118,foot,118,'文档数据',kind='primary');return
 if s=='提交结果未知':
  x,y,w,foot=panel(b,'这次保存的结果尚未确认','不会自动重发原始提交。',h=446)
  notice(b,'保留原始提交','片段 02 · 使用原生的控件与尺度 · 版本 4',x=x,y=y-6,w=w,warning=True)
  label(b,'原内容和本地修改都已保留。\n先读取当前版本，核对这次保存是否已经成功，\n再决定后续处理。',x,y+88,w,13,'#626262',h=90)
  button(b,'保留并返回',x,foot,131,'片段编辑',kind='bordered');button(b,'核对当前版本',x+w-144,foot,144,'片段保存',kind='primary');return
 # Visible content states share the source module's scope and return routes.
 if s=='筛选为空':
  shape(b,'empty/cover',486,101,794,719,'#ffffff');chip(b,'等待外部  0',507,115,104,True)
  symbol_icon(b,'filter',868,322,31,'#9a9a9a');label(b,'没有符合筛选的事项',767,382,370,17,INK,h=31,weight='semibold');label(b,'清除标签，查看本群全部待办。',778,425,350,12,MUTED,h=29);button(b,'清除筛选',848,476,119,'人待办',kind='bordered');return
 shape(b,'reading/cover',486,101,794,719,'#ffffff')
 if s=='读取中':
  symbol_icon(b,'refresh',875,325,28,'#a5a5a5');label(b,'正在读取本群文档…',796,384,340,13,MUTED,h=26);button(b,'取消',858,433,90,'文档数据',kind='bordered')
 else:
  symbol_icon(b,'warning',875,306,29,'#a17c3c');label(b,'暂时无法读取文档',785,363,370,17,INK,h=31,weight='semibold');label(b,'连接已断开。已保存的内容不会被清空。',741,409,450,12,MUTED,h=32);button(b,'返回消息',784,470,113,'群聊',kind='bordered');button(b,'重新读取',910,470,119,'读取中',kind='primary')


SIDEBAR_STATES={'列表折叠':'群聊','会话选择·人待办':'人待办','会话选择·AI待办':'AI待办','会话选择·文档':'文档数据','会话选择·进展':'计划进展','后端会话':'另一机组'}
def shell(s,b,selected='群聊'):
 actual=SIDEBAR_STATES.get(s,selected)
 legacy_shell(s,b,'群聊' if s=='后端会话' else actual)
 # Only the full-height sidebar remains; old middle-column atoms are removed.
 b['layers']=[l for l in b['layers'] if not (238<=l['frame']['x']<486 and l['frame']['y']>=54) and l['name']!='workspace/底部分隔']
 if s in ['新建机组','填写机组','空白']:return
 pageTarget={'人待办':'会话选择·人待办','AI待办':'会话选择·AI待办','文档数据':'会话选择·文档','计划进展':'会话选择·进展'}.get(actual,'列表折叠')
 if s not in SIDEBAR_STATES:
  if s=='另一机组':
   for layer in b['layers']:
    if layer['name']=='sidebar/选中机组':layer['frame']['y']+=70
  # The row is a normal crew selector. Trailing arrow pushes its sidebar subpage.
  for l in b['layers']:
   if l['name'].startswith('sidebar/机组 '):
    target='另一机组' if '后端实现' in l['name'] else '群聊';l['flow']['destinationArtboardID']=boards[target]['do_objectID']
   if l['_class']=='text' and l['frame']['x']==177:l['attributedString']['string']=''
  for i in range(4):symbol_icon(b,'chevron.right',207,124+i*70,14,'#8b8b8b','后端会话' if i==1 else pageTarget)
  symbol_icon(b,'ellipsis',181,191 if s=='另一机组' else 121,18,'#9b9b9b','机组详情')
 else:
  shape(b,'sidebar/subpage background',0,54,238,700,'#efeeeb')
  symbol_icon(b,'chevron.left',18,70,17,'#575757','另一机组' if s=='后端会话' else actual);label(b,'机组',40,72,64,12,'#626262',h=22);hotspot(b,'返回机组选择并保留上下文',12,61,92,35,'另一机组' if s=='后端会话' else actual)
  label(b,'会话',17,117,180,17,h=30,weight='semibold');label(b,'后端实现' if s=='后端会话' else 'PendingCrew',17,151,198,11,MUTED,h=22);symbol_icon(b,'plus',204,119,18,MUTED,'新建机组')
  rows=[('验证讨论' if s=='后端会话' else '大厅','群聊 · 4 位成员','group','另一机组' if s=='后端会话' else '群聊'),('与你沟通','你与机长','captain','单聊'),('后端','Codex · 工作中','backend','单聊'),('设计','Codex · 等待消息','design','单聊')]
  for i,(title,sub,av,target) in enumerate(rows):
   yy=188+i*67
   if i==0:shape(b,'sidebar/channel selection',12,yy,214,58,'#e2e7e3',8)
   avatar(b,av,22,yy+12,32);label(b,title,65,yy+9,153,13,h=22,weight='medium');label(b,sub,65,yy+33,153,10,MUTED,h=19);hotspot(b,'sidebar/conversation '+title,12,yy,214,58,target)

CODEX_PAGES={'Codex控制':'概览','Codex执行':'执行','Codex历史':'历史','Codex设置':'设置','Codex高级设置':'设置','模型选择':'设置','Codex请求答复':'执行','Codex控制不可用':'概览','Codex操作确认':'历史','Codex输入选择':'执行','Codex删除确认':'历史','Codex截断确认':'历史','Codex排队回执':'执行'}
def codex_frame(b,s):
 x,y,w,h=419,35,680,760
 glass(b,'native/Codex control sheet',x,y,w,h,14);symbol_icon(b,'terminal',x+20,y+23,21,MUTED);label(b,'Codex 会话',x+52,y+20,530,17,h=30,weight='semibold');label(b,'查看状态与历史不会恢复会话。',x+20,y+57,620,11,MUTED,h=24);symbol_icon(b,'xmark',x+w-36,y+26,16,MUTED,'back')
 shape(b,'codex/pages segment',x+20,y+94,320,29,'#f0f0f0',7)
 for i,(title,target) in enumerate([('概览','Codex控制'),('执行','Codex执行'),('历史','Codex历史'),('设置','Codex设置')]):
  xx=x+21+i*79
  if CODEX_PAGES[s]==title:shape(b,'codex/current page',xx,y+95,78,27,'#ffffff',6,'#e4e4e4')
  label(b,title,xx+25,y+101,55,12,h=20,weight='medium' if CODEX_PAGES[s]==title else 'regular');hotspot(b,'Codex/'+title,xx,y+94,78,29,target)
 shape(b,'codex/page divider',x,y+139,w,.6,LINE)
 return x+20,y+159,w-40,y+h-45

def setting_check(b,title,value,x,y,w=640,target=None,on=False):
 shape(b,'checkbox/'+title,x,y+3,13,13,'#63826f' if on else '#ffffff',3,'#c9c9c9')
 if on:symbol_icon(b,'check',x+2,y+5,9,'#ffffff')
 label(b,title,x+24,y,w-25,12,h=22,weight='medium');label(b,value,x+25,y+25,w-26,11,MUTED,h=27)
 if target:hotspot(b,'setting/'+title,x,y,w,53,target)

def full_codex(b,s):
 x,y,w,foot=codex_frame(b,s)
 if s=='Codex排队回执':
  notice(b,'已明确排入下一轮','原文与独立输入保持绑定；接受不等于已投递或完成。',x=x,y=y,w=w)
  label(b,'队列：1 项',x,y+106,w,14,h=30,weight='semibold');label(b,'先整理这次验证的证据，暂不继续下一步实现。',x,y+153,w,13,h=57);label(b,'停止整个会话会放弃余下队列。',x,y+235,w,12,MUTED,h=34)
  button(b,'返回执行',x,foot,125,'Codex执行',kind='bordered');return
 if s=='Codex输入选择':
  label(b,'独立输入与 Skills',x,y,w,14,h=26,weight='semibold');label(b,'添加仅保存原输入，不会发送任务。',x,y+34,w,11,MUTED,h=25)
  button(b,'选择输入文件',x,y+73,159,'Codex输入选择',kind='bordered');label(b,'图片、音频和 UTF-8 文本；不支持 PDF / 其他二进制。',x,y+116,w,11,MUTED,h=29)
  setting_check(b,'design-notes.txt','本机暂存 · 已准备 · 合成示例',x,y+167,w,'Codex输入选择',True)
  button(b,'丢弃原暂存',x+25,y+230,137,'Codex输入选择',kind='bordered');button(b,'导出本机原文件',x+177,y+230,176,'Codex输入选择',kind='bordered')
  label(b,'Skill 目录尚未读取',x,y+303,w,13,h=26,weight='medium');button(b,'读取当前目录可用 Skills',x,y+350,230,'Codex控制不可用',kind='bordered')
  label(b,'只选择当前原目录实际启用条目；目录变化会拒绝，不代换路径。\n未知暂存结果只查原 UUID，不重传。',x,y+405,w,11,MUTED,h=59)
  button(b,'重新读取本机选择',x,foot,185,'Codex输入选择',kind='bordered');button(b,'返回执行',x+w-128,foot,128,'Codex执行',kind='primary');return
 if s in ['Codex删除确认','Codex截断确认']:
  danger=s=='Codex删除确认';label(b,'永久删除原生历史？' if danger else '截断所选原生历史？',x,y,w,17,h=33,weight='semibold')
  notice(b,'删除不可逆，不能撤销' if danger else '只保留所选轮次之前的历史','PCW 工作、消息、原回执与工作文件保留；不回滚文件。',x=x,y=y+70,w=w,warning=True)
  label(b,'原会话代际与请求编号保持绑定。\n如果结果未知，只核对原请求，不自动重发。',x,y+181,w,13,h=68)
  button(b,'取消',x,foot,100,'Codex历史',kind='bordered');button(b,'永久删除原生历史' if danger else '确认截断原生历史',x+w-203,foot,203,'Codex历史',kind='danger');return
 if s=='Codex控制不可用':
  notice(b,'当前后台未接入此控制协议','现有消息与明确恢复入口仍可使用。',x=x,y=y,w=w,warning=True)
  label(b,'能力随当前后台实际返回显示，缺失值保持未知。',x,y+95,w,12,MUTED,h=35)
  for i,title in enumerate(['读取模型目录','Steer 当前轮次','建立分叉','提交后续轮次设置']):button(b,title,x,y+155+i*46,228,kind='bordered',disabled=True)
  button(b,'返回消息',x,foot,123,'群聊',kind='bordered');return
 page=CODEX_PAGES[s]
 if page=='概览':
  label(b,'当前会话',x,y,w,14,h=26,weight='semibold')
  for i,(k,v) in enumerate([('执行状态','已运行 · 空闲'),('队列','0 项'),('工作目录','PendingCrew'),('期望标题','整理验证记录'),('原生标题','未确认 · 保留未知')]):
   yy=y+39+i*36;label(b,k,x,yy,110,11,MUTED,h=20);label(b,v,x+126,yy,w-130,12,h=23)
  field(b,'明确设置会话名','整理验证记录',x,y+234,w);button(b,'同步原生标题',x,y+303,154,'Codex控制',kind='bordered')
  label(b,'▾ 上下文用量、工具与 Skills',x,y+366,w,13,h=26,weight='medium');label(b,'上下文窗口：未观察到  ·  最近轮次 Token：未知',x,y+402,w,11,MUTED,h=26)
  button(b,'读取原始用量',x,y+445,151,'Codex控制不可用',kind='bordered');button(b,'读取工具与 Skill 元数据',x+165,y+445,231,'Codex控制不可用',kind='bordered')
  label(b,'读取清单不调用工具或安装插件。缺失值不补成 0。',x,y+490,w,11,MUTED,h=27)
  button(b,'手动恢复原会话',x+w-171,foot,171,'恢复确认',kind='primary');button(b,'能力与原请求核对',x,foot,192,'Codex控制不可用',kind='bordered');return
 if page=='执行':
  if s=='Codex请求答复':
   notice(b,'有一项请求等待你的答复','当前会话 · 原结构化请求 · 不会自动替你选择。',x=x,y=y,w=w,warning=True)
   label(b,'工作区文件写入请求',x,y+96,w,14,h=27,weight='semibold');label(b,'请求操作：修改当前工作目录中的设计文件。\n请求范围：当前会话与本次操作。',x,y+137,w,13,h=64)
   field(b,'你的补充答复（可选）','只允许当前工作目录中的本次修改。',x,y+230,w)
   button(b,'拒绝',x,y+328,99,'Codex执行',kind='bordered');button(b,'允许本次',x+112,y+328,124,'Codex执行',kind='primary');button(b,'保持待答复',x+251,y+328,147,'back',kind='bordered')
   label(b,'示例表单；真实字段与可选答案由原请求提供。',x,y+403,w,11,MUTED,h=27);return
  label(b,'当前执行与后续任务',x,y,w,14,h=26,weight='semibold');shape(b,'codex/input text',x,y+39,w,75,'#ffffff',7,'#d7d7d7');label(b,'先整理这次验证的证据，暂不继续下一步实现。',x+12,y+53,w-24,13,INK,h=45)
  label(b,'▸ 当前轮次的独立输入与 Skills',x,y+135,w,12,h=24);hotspot(b,'当前轮次独立输入',x,y+130,w,30,'Codex输入选择');button(b,'Steer 当前轮次',x,y+171,167,'Codex请求答复',kind='bordered',disabled=True)
  label(b,'▸ 下一轮的独立输入与 Skills',x,y+221,w,12,h=24);hotspot(b,'下一轮独立输入',x,y+216,w,30,'Codex输入选择');button(b,'明确排入下一轮',x,y+257,175,'Codex排队回执',kind='bordered');label(b,'排队接受不代表已投递或完成；停止会放弃余下队列。',x,y+301,w,11,MUTED,h=27)
  button(b,'打断当前轮次',x,y+347,151,'停工确认',kind='bordered',disabled=True);button(b,'停止整个会话',x+163,y+347,151,'停工确认',kind='bordered');button(b,'手动恢复',x+326,y+347,123,'恢复确认',kind='bordered')
  button(b,'压缩上下文',x,y+398,139,'Codex操作确认',kind='bordered');field(b,'审查目标','未提交改动  ▾',x,y+450,w,'menu','Codex操作确认');button(b,'开始审查',x+w-122,foot,122,'Codex操作确认',kind='primary');button(b,'查看待答复请求',x,foot,189,'Codex请求答复',kind='bordered');return
 if page=='历史':
  label(b,'会话历史',x,y,w,14,h=26,weight='semibold');label(b,'读取不恢复会话，查看旧轮次不回滚文件。',x,y+31,w,11,MUTED,h=24);button(b,'读取原生历史',x,y+69,157,'Codex历史',kind='bordered')
  for i,(title,sub) in enumerate([('turn-02 · 已完成','整理现有验证证据与资料关联'),('turn-01 · 已完成','读取原始需求与目录上下文')]):
   yy=y+121+i*100;label(b,title,x,yy,w,12,h=23,weight='medium');label(b,sub,x,yy+29,w,11,MUTED,h=24);button(b,'选为分叉起点',x+367,yy+9,150,'Codex操作确认',kind='plain');button(b,'在此之前截断',x+367,yy+43,150,'Codex截断确认',kind='plain')
  field(b,'新会话名称（可留空）','未指定',x,y+334,w);button(b,'建立停止状态的分叉',x,y+404,215,'Codex操作确认',kind='bordered');label(b,'分叉不会启动源或新会话，需另行明确恢复。',x,y+445,w,11,MUTED,h=25)
  button(b,'归档原生历史',x,y+491,154,'Codex操作确认',kind='bordered');button(b,'取消归档',x+167,y+491,114,'Codex操作确认',kind='bordered');button(b,'永久删除历史',x+w-161,y+491,161,'Codex删除确认',kind='danger')
  if s=='Codex操作确认':
   xx,yy,ww,ff=panel(b,'确认执行这个会话操作？','原生历史操作绑定当前已读取的原会话代际。',x=493,y=276,w=533,h=325)
   label(b,'执行来源按钮所选的原会话操作。\n分叉与归档不会启动会话；压缩与审查的回执仅表示开始。\n未知结果只核对原请求，保留本机原文。',xx,yy,ww,12,'#626262',h=94)
   button(b,'取消',xx+250,ff,83,'back',kind='bordered');button(b,'确认操作',xx+345,ff,119,'Codex历史',kind='primary')
  return
 # Settings is a real source-shaped editor; model/effort are independent.
 if s=='模型选择':
  label(b,'实际模型目录',x,y,w,14,h=26,weight='semibold');notice(b,'尚未读取模型目录','未观察到真实模型 ID，不提供臆造的型号或 effort 组合。',x=x,y=y+44,w=w)
  button(b,'读取实际模型目录',x,y+142,191,'Codex控制不可用',kind='bordered');label(b,'收到目录后按每个模型实际支持的推理强度分别展示。',x,y+199,w,12,MUTED,h=50);button(b,'返回设置',x,foot,121,'Codex设置',kind='bordered');return
 if s=='Codex高级设置':
  label(b,'权限与高级设置',x,y,w,14,h=26,weight='semibold');label(b,'后续轮次草稿 · 尚未提交，不代表当前进程已应用。',x,y+31,w,11,MUTED,h=25)
  rows=[('设置审批策略','逐类控制',True),('逐类控制','',False),('设置审批者','用户',True),('权限 Profile','由原目录选择；与沙箱二选一',False),('设置沙箱','工作区可写',True),('允许网络','关闭',False),('设置推理摘要','自动',True)]
  for i,(title,value,on) in enumerate(rows):
   yy=y+73+i*59;shape(b,'checkbox/'+title,x,yy+4,13,13,'#63826f' if on else '#ffffff',3,'#c9c9c9')
   if on:symbol_icon(b,'check',x+2,yy+6,9,'#ffffff')
   label(b,title,x+24,yy,w-25,12,h=23,weight='medium')
   if i==1:
    for j,t in enumerate(['MCP','权限','规则','沙箱','Skill']):
     xx=x+157+j*88;shape(b,'granular/'+t,xx,yy+4,12,12,'#63826f',3);symbol_icon(b,'check',xx+2,yy+6,8,'#ffffff');label(b,t,xx+18,yy+1,65,11,h=21);hotspot(b,'逐类/'+t,xx,yy,83,25,'Codex高级设置')
   else:
    shape(b,'picker/'+title,x+208,yy-2,w-208,28,'#ffffff' if on else '#f7f7f7',6,'#dddddd');label(b,value,x+217,yy+5,w-243,12,INK if on else '#a5a5a5',h=22)
    if i not in [3,5]:symbol_icon(b,'chevron.down',x+w-23,yy+7,11,MUTED)
    hotspot(b,'高级设置/'+title,x,yy,w,41,'Codex控制不可用' if i==3 else 'Codex高级设置')
  label(b,'审批可选：按请求 / 不可信 / 不询问 / 逐类；摘要：自动 / 简洁 / 详细 / 无。',x,y+500,w,10,MUTED,h=27)
  button(b,'读取原目录权限 Profiles',x,foot,240,'Codex控制不可用',kind='bordered');button(b,'提交后续轮次设置',x+w-199,foot,199,'Codex设置',kind='primary');return
 label(b,'实际观察与后续轮次期望',x,y,w,14,h=26,weight='semibold');label(b,'已观察模型：未返回  ·  设置状态：未提交',x,y+32,w,11,MUTED,h=24);button(b,'读取实际模型目录',x,y+70,190,'模型选择',kind='bordered')
 for i,(t,v,target) in enumerate([('指定模型','沿用原设置；从实际目录选取','模型选择'),('指定推理强度','沿用原设置；只使用模型支持值','模型选择'),('设置速度档位','后续轮次草稿；未应用','Codex设置'),('设置协作模式','默认 / 计划；未应用','Codex设置')]):setting_check(b,t,v,x,y+127+i*74,w,target,t in ['设置速度档位','设置协作模式'])
 shape(b,'Fast switch track',x+486,y+282,32,19,'#6a8a76',10);ellipse(b,'Fast switch thumb',x+501,y+284,15,15,'#ffffff',True);label(b,'Fast',x+528,y+283,73,11,h=21);shape(b,'collaboration menu',x+361,y+350,279,28,'#ffffff',6,'#d8d8d8');label(b,'默认  ▾',x+374,y+357,251,12,h=23);button(b,'权限与高级设置',x,y+437,189,'Codex高级设置',kind='bordered');label(b,'未启用的项目沿用原设置，界面默认值不冒充实际观察。',x,y+488,w,11,MUTED,h=28);button(b,'提交后续轮次设置',x+w-199,foot,199,'Codex设置',kind='primary')

for s,b in boards.items():
 selected='人待办' if s in ['人待办','编辑待办','待办完成','待办分组','筛选为空'] else 'AI待办' if s.startswith('AI') else '文档数据' if s in ['文档数据','片段编辑','片段保存','关联示例','生成页面','页面预览','读取中','读取失败','只读权限','提交结果未知'] else '计划进展' if s in ['计划进展','步骤展开','进展折叠'] else '群聊'
 selected=SIDEBAR_STATES.get(s,selected)
 shell(s,b,selected)
 if s=='空白':
  glass(b,'空态/唯一玻璃加号',731,383,56,56,28);symbol_icon(b,'plus',746,398,26,MUTED,'新建机组');continue
 if s in ['新建机组','填写机组']:
  button(b,'取消',1197,65,65,'群聊');composer(b,'开始整理这次的设计方向。' if s=='填写机组' else '发消息以新建机组',s=='填写机组',True);continue
 if selected=='人待办':
  if s=='待办完成':completed(b)
  else:todo(b,s)
 elif selected=='AI待办':ai_todo(b,s)
 elif selected=='文档数据':document(b,s)
 elif selected=='计划进展':progress(b,s)
 else:
  if s in ['单聊','另一机组','已创建机组','重新打开','后端会话']:
   chatbar(b)
   if s=='单聊':bubble(b,'验证记录已经整理好了。\n我把证据和恢复步骤放在同一份文档里。',162,name='后端',av='backend',height=75);bubble(b,'收到，先保留现场。',304,True,height=49);chip(b,'文档 · 重连验证记录',549,407,176,False,'文档数据')
   elif s in ['另一机组','后端会话']:bubble(b,'这里是后端实现机组。\n恢复点已保留，等你安排下一步。',162,name='后端',av='backend',height=76);bubble(b,'请先整理验收证据，暂不继续执行。',310,True,height=52)
   elif s=='已创建机组':bubble(b,'开始整理这次的设计方向。',162,True,height=52);bubble(b,'京都已创建。我们先把需求放在这段对话里。',285,height=61)
   else:notice(b,'会话已恢复 · 尚未执行','重新打开不会自动继续。核对上下文后再手动恢复。',y=141);button(b,'检查并手动恢复',543,235,183,'恢复确认',kind='bordered')
   composer(b)
  else:chat(b,s)
 overlay_start=len(b['layers'])
 if s=='编辑待办':human_editor(b)
 if s=='待办分组':menu_panel(b,'标签与分组',[('所有待办','check','人待办'),('设计确认',None,'人待办'),('首版验收',None,'人待办'),('等待外部 · 0',None,'筛选为空'),('新建分组…','plus','编辑待办')],x=653,y=144,w=271)
 if s in ['片段编辑','片段保存','关联示例','生成页面','页面预览']:doc_overlay(b,s)
 if s in ['成员','成员详情']:members(b,s)
 if s in ['消息分类','分类展开']:menu_panel(b,'本群消息分类',[('全部消息','check','群聊'),('实现讨论','folder','分类展开'),('  验证讨论',None,'群聊'),('界面讨论','folder','群聊'),('管理分类…','pencil','待办分组')],x=830,y=97,w=264)
 if s in ['搜索','全局搜索','搜索结果']:search_panel(b,s)
 if s=='机器菜单':menu_panel(b,'机器',[('Mac Studio','check','群聊'),('MacBook Pro',None,'机器切换'),('添加机器…','plus','连接设置'),('设置…','gear','设置'),('退出 PendingCrew…',None,'退出询问')],x=15,y=537,w=223)
 if s=='机器切换':
  x,y,w,foot=panel(b,'MacBook Pro','远端电脑 · 离线',h=347);symbol_icon(b,'warning',x,y,27,MUTED);label(b,'连接未建立。\n此电脑的机组和内容不会与本机混合。',x+43,y,w-48,13,'#626262',h=79);button(b,'连接设置',x,foot,130,'连接设置',kind='bordered');button(b,'返回 Mac Studio',x+w-155,foot,155,'群聊',kind='primary')
 if s in ['设置','连接设置','通知设置']:settings(b,s)
 if s=='机组详情':menu_panel(b,'PendingCrew · 机组',[('机组目录：PendingCrew','folder','群聊'),('父机组：PendingName',None,'通讯录'),('新建子机组…','plus','新建机组'),('成员与工作','people','成员'),('Codex 会话…','terminal','Codex控制')])
 if s in CODEX_PAGES:full_codex(b,s)
 if s in ['恢复确认','已恢复','停工确认','已停工','安全暂停','关闭等待窗','强制关闭']:confirmation(b,s)
 if s in ['退出询问','等待停工']:quit_panel(b,s)
 if s in ['读取中','读取失败','只读权限','提交结果未知','筛选为空']:boundaries(b,s)
 for layer in b['layers'][overlay_start:]:layer['userInfo']={'refinedOverlay':True,'fixedNavigation':s in CODEX_PAGES}

# Real main list is 196pt; an independent 18pt fold edge follows it.
# Keep right-anchored toolbar and composer controls fixed while reclaiming width.
for state,board in boards.items():
 if state in ['新建机组','填写机组']:continue
 for layer in board['layers']:
  f=layer['frame']
  if layer.get('userInfo',{}).get('fixedNavigation') or layer.get('userInfo',{}).get('anchoredToolbar'):continue
  if f['x']>=486 and (f['x']<1100 or layer.get('userInfo',{}).get('refinedOverlay')):f['x']-=34
  if layer['name'] in ['composer/input pill','聊天/工具栏分隔','todo/分隔','资料/分隔','reading/cover','empty/cover','web/清除文档内容']:f['width']+=34


for state in ['另一机组','后端会话']:
 for layer in boards[state]['layers']:
  if layer['_class']=='text' and layer['attributedString']['string']=='大厅':layer['attributedString']['string']='验证讨论'

# Reclaim the former conversation column for the shared right work surface.
for state,board in boards.items():
 for layer in board['layers']:
  f=layer['frame'];name=layer['name'];info=layer.get('userInfo',{})
  if state in CODEX_PAGES and info.get('fixedNavigation'):
   # Codex pages are authored directly at the new, centered 680pt frame.
   continue
  if state in ['新建机组','填写机组','空白']:continue
  if f['x']>=452 and f['y']>=54 and not info.get('anchoredToolbar'):
   if info.get('refinedOverlay'):f['x']-=148
   elif f['x']<1100 and not name.startswith('消息/你') and not info.get('anchoredRight'):f['x']-=214
  if name in ['composer/input pill','聊天/工具栏分隔','todo/分隔','资料/分隔','progress/separator','todo/当前事项','reading/cover','empty/cover','web/清除文档内容']:f['width']+=214
  if name=='空态/唯一玻璃加号':f['x']=731
 # Unselected message/group single-click never starts a session.
# Organize native editable layers, including interactive children, by ownership.
for state,board in boards.items():
 buckets={k:[]for k in ['00 Window','01 Sidebar','02 Workspace Toolbar','04 Content','05 Popovers & Sheets']}
 for layer in board['layers']:
  f=layer['frame'];name=layer['name']
  key='05 Popovers & Sheets' if layer.get('userInfo',{}).get('refinedOverlay') else '00 Window' if name=='window/白色共用画布' else '01 Sidebar' if f['x']<238 else '02 Workspace Toolbar' if f['y']<55 else '04 Content'
  buckets[key].append(layer)
 grouped=[]
 for name,layers in buckets.items():
  if not layers:continue
  g=base('Group',name,0,0,W,H);g['layers']=layers;grouped.append(g)
 board['layers']=grouped

page=default(json.loads((P/'schema-source/package/dist/page.schema.json').read_text()));page.update(name='SwiftUI 精修 · 侧栏会话导航与完整控制',isVisible=True,booleanOperation=-1,resizingConstraint=63,layers=list(boards.values()))
comp=default(json.loads((P/'schema-source/package/dist/page.schema.json').read_text()));comp.update(name='Native Components · 可复用 Symbols',isVisible=True,booleanOperation=-1,resizingConstraint=63,layers=list(masters.values()))
for i,m in enumerate(comp['layers']):m['frame'].update(x=i%5*250,y=i//5*110)
doc=default(schema['properties']['document']);doc['pages']=[{'_class':'MSJSONFileReference','_ref_class':'MSImmutablePage','_ref':'pages/'+p['do_objectID']}for p in [page,comp]]
meta=obj('Meta');meta.update(version=146,appVersion='2026.3',build=234013,commit='native-refinement',pagesAndArtboards={p['do_objectID']:{'name':p['name'],'artboards':{b['do_objectID']:{'name':b['name']}for b in p['layers']}}for p in [page,comp]});meta['created'].update(version=146,appVersion='2026.3',build=234013)
filename='PendingCrew-SwiftUI-Four-Proofs.sketch' if PROOF else 'PendingCrew-SwiftUI-Sidebar-Navigation.sketch'
with zipfile.ZipFile(P/filename,'w',zipfile.ZIP_DEFLATED)as z:
 for n,v in [('document.json',doc),('meta.json',meta),('user.json',obj('User'))]+[('pages/'+p['do_objectID']+'.json',p)for p in [page,comp]]:z.writestr(n,json.dumps(v,ensure_ascii=False))
print(filename,len(boards),'boards',len(masters),'Symbols',len(flows),'Flow links')

(P/'sidebar-navigation-profile.json').write_text(json.dumps({'file':filename,'artboards':len(boards),'stateNames':list(boards),'symbols':len(masters),'sourceGraph':'original 49 states retained; five boundary states; Codex four-page controls and sidebar context states','sidebar':238,'standaloneConversationColumn':False,'topTabDivider':False,'nativeDoubleClick':False,'sha256':hashlib.sha256((P/filename).read_bytes()).hexdigest()},ensure_ascii=False,indent=2))
