# Current design, extending the 2026-10-13 native layer generator and its schema.
from sketch_helpers import *
from copy import deepcopy
STATES=['群聊','单聊','另一机组','通讯录','机器切换','空白','新建机组','填写机组','已创建机组','成员','成员详情','消息分类','分类展开','列表折叠','人待办','待办完成','编辑待办','待办分组','AI待办','AI待办详情','文档数据','片段编辑','片段保存','关联示例','生成页面','页面预览','计划进展','步骤展开','进展折叠','搜索','全局搜索','搜索结果','机器菜单','设置','连接设置','通知设置','机组详情','Codex控制','模型选择','恢复确认','已恢复','停工确认','已停工','退出询问','安全暂停','等待停工','关闭等待窗','强制关闭','重新打开']
boards={s:base('Artboard',f'{i+1:02d} {s}',i%6*1480,i//6*940,1440,860) for i,s in enumerate(STATES)}
links=[]
def add(b,a):b['layers'].append(a);return a
def txt(b,s,x,y,w=500,size=14,c='#26352f',h=30,bold=False):
 a=text(s,x,y,w,size,c,h)
 if bold:a['attributedString']['attributes'][0]['attributes']['MSAttributedStringFontAttribute']['attributes']['name']='PingFangSC-Semibold'
 return add(b,a)
def box(b,n,x,y,w,h,fill='#ffffff',r=10,target=None):
 a=add(b,rect(n,x,y,w,h,fill,r))
 if target:
  a['flow']={'_class':'MSImmutableFlowConnection','destinationArtboardID':'back' if target=='back' else boards[target]['do_objectID'],'animationType':0,'maintainScrollPosition':True};links.append([b['name'],n,target])
 return a
def btn(b,s,x,y,w=110,target=None,active=False,h=32):
 box(b,s,x,y,w,h,'#e6e6e6' if active else '#f1f1f1',8,target);txt(b,s,x+10,y+6,w-18,13)
def row(b,s,sub,x,y,w=276,target=None,sel=False,h=74):
 box(b,s,x,y,w,h,'#e7e7e7' if sel else '#fafafa',9,target);txt(b,s,x+13,y+10,w-26,14,bold=sel);txt(b,sub,x+13,y+39,w-26,11,'#7b8780')
def bubble(b,s,y,sender='机长',own=False,target=None):
 x=990 if own else 660;w=355 if own else 570
 box(b,'头像 '+sender,1360 if own else 614,y,30,30,'#e2e9e1',8);txt(b,sender[:1],1367 if own else 623,y+4,24)
 txt(b,sender,x,y-23,w,11,'#818c85');box(b,'消息 '+s,x,y,w,66,'#c8e8ce' if own else '#ffffff',10,target);txt(b,s,x+16,y+15,w-32,14,h=48)
def modal(b,title,lines,actions):
 box(b,'浮层底色',595,175,710,498,'#e8ede9',20);box(b,'浮层',599,171,702,494,'#ffffff',18)
 txt(b,title,630,202,610,23,bold=True);btn(b,'关闭',1205,196,70,'back')
 for i,line in enumerate(lines):txt(b,line,630,268+i*44,610,15,h=44)
 for i,(label,target) in enumerate(actions):btn(b,label,630+i*210,595,198,target,i==0)
def shell(s,b):
 b.update(hasBackgroundColor=True,includeBackgroundColorInExport=True,backgroundColor=color('#fafafa'),isFlowHome=s=='群聊')
 box(b,'全窗',0,0,1440,860,'#fafafa',0);box(b,'全高侧栏',0,0,216,860,'#f5f5f5',0)
 txt(b,'●  ●  ●',19,17,155,12,'#b5bdb7');txt(b,'',20,59,176,21,bold=True)
 btn(b,'消息',16,108,86,'群聊',s!='通讯录');btn(b,'通讯录',110,108,90,'通讯录',s=='通讯录')
 if s=='通讯录':
  for i,(t,sub,target) in enumerate([('▾ 🖥️ Mac Studio','本机 · 在线','群聊'),('  ▾ PendingName','私人团队','群聊'),('     ▾ PendingCrew','2 个子机组','群聊'),('        后端实现','父：PendingCrew','另一机组'),('        前端设计','父：PendingCrew','群聊'),('  ▾ 产品讨论','机组','群聊'),('        前端设计 ↗','同一机组的父引用','群聊'),('▸ 💻 MacBook Pro','远端机器 · 离线','机器切换')]):row(b,t,sub,12,162+i*74,192,target,h=69)
 else:
  txt(b,'机组',20,169,160,11,'#7b8780')
  for i,(t,sub,target) in enumerate([('PendingCrew','私人团队 · 本机','机组详情'),('  后端实现','子机组','另一机组'),('  前端设计','子机组','群聊'),('PendingName','私人团队','另一机组')]):row(b,t,sub,12,196+i*82,192,target,sel=i==0)
  btn(b,'＋ 新建机组',16,547,184,'新建机组')
 box(b,'机器菜单触发',12,786,192,58,'#f5f5f5',12,'机器菜单');box(b,'浅圆emoji底',22,795,38,38,'#eeeeee',19);txt(b,'🖥️',27,802,35,22);txt(b,'Mac Studio',68,809,94,14,bold=True);txt(b,'⌃',165,803,24,25,bold=True);txt(b,'●',192,812,12,9,'#4d9a6b')
 # One tab strip spans both the conversation column and chat, at the very top.
 box(b,'工作区顶部',216,0,1224,61,'#ffffff',0)
 box(b,'连续胶囊Tabs',230,10,570,42,'#f1f1f1',21)
 for i,(t,target) in enumerate([('消息','群聊'),('人待办','人待办'),('AI待办','AI待办'),('文档数据','文档数据'),('计划进展','计划进展')]):
  if s==target:box(b,'选中Tab',234+i*112,14,110,34,'#ffffff',17)
  box(b,'Tab热点 '+t,234+i*112,14,110,34,'#ffffff' if s==target else '#f1f1f1',17,target);txt(b,t,251+i*112,21,85,13)
 box(b,'液态玻璃搜索',1380,10,42,42,'#f0f0f0',21,'搜索');txt(b,'⌕',1391,15,30,24)
 collapsed=s=='列表折叠';cx=232 if collapsed else 600
 if not collapsed:
  box(b,'会话列',216,62,368,798,'#fafafa',0);txt(b,'最近会话',237,82,230,16,bold=True)
  btn(b,'＋',529,78,38,'新建机组')
  for i,(n,sub,target) in enumerate([('👥 PendingCrew','机长：今天先把团队界面定下来','群聊'),('🧑‍💻 后端 · PendingCrew','验证记录已经整理好了','单聊'),('👥 后端实现','已保留恢复点，等你安排','另一机组'),('🎨 设计 · PendingCrew','我们保持原来的轻玻璃布局','单聊')]):row(b,n,sub,228,127+i*87,344,target,sel=(s=='另一机组' and i==2) or (s=='单聊' and i==1) or (s not in ['单聊','另一机组'] and i==0),h=80)
  btn(b,'‹',568,422,24,'列表折叠',h=42)
 else:btn(b,'›',218,422,24,'群聊',h=42)
 box(b,'聊天画布',cx,62,1440-cx,798,'#fafafa',0)
 return cx

chatstates=['群聊','单聊','另一机组','通讯录','机器切换','成员','成员详情','消息分类','分类展开','列表折叠','搜索','全局搜索','搜索结果','机器菜单','设置','连接设置','通知设置','机组详情','Codex控制','模型选择','恢复确认','已恢复','停工确认','已停工','退出询问','安全暂停','等待停工','关闭等待窗','强制关闭','重新打开']
for s,b in boards.items():
 cx=shell(s,b)
 if s=='空白':
  box(b,'玻璃加号底',955,386,94,94,'#e8eee7',28,'新建机组');txt(b,'＋',977,396,70,55,'#8fa496',h=80)
  continue
 title='后端实现' if s=='另一机组' else '后端' if s=='单聊' else '新建机组' if s in ['新建机组','填写机组'] else '京都' if s=='已创建机组' else 'PendingCrew'
 # Conversation title and main-panel details button intentionally omitted.
 if s in chatstates or s=='已创建机组':
  if s!='单聊':btn(b,'仅 @你',cx+24,86,96,'群聊');btn(b,'成员 4',cx+128,86,102,'成员')
  btn(b,'消息分类 ▾',cx+240,86,150,'消息分类')
  if s=='重新打开':
   row(b,'会话已恢复 · 尚未执行','重开应用不会自动继续。你可以检查上下文后手动恢复。',cx+24,221,1440-cx-48,'恢复确认',h=92)
  elif s=='另一机组':
   bubble(b,'后端实现：恢复点已保留，这里的内容只属于本机组。',270,'后端');bubble(b,'等待你的下一条安排。',385,'后端')
  elif s=='单聊':
   bubble(b,'验证记录已经整理好了。',270,'后端');bubble(b,'收到，先保留现场。',385,'你',True)
  elif s=='已创建机组':bubble(b,'开始整理这次的设计方向。',270,'你',True);bubble(b,'京都已创建，我会先整理需求。',385)
  else:
   bubble(b,'今天先把团队界面定下来。',270,'你',True);bubble(b,'收到，消息、人和资料都留在各自机组里。',385);bubble(b,'@你 还有一项待办需要决定。',500,'后端',target='人待办')
  box(b,'输入区',cx,667,1440-cx,193,'#ffffff',0);txt(b,'＋    附件    @    引用',cx+24,684,500,15,'#819086');txt(b,'发消息…',cx+24,730,500,15,'#a0aaa1');btn(b,'发送',1315,807,100,'群聊',True)
 elif s in ['新建机组','填写机组']:
  box(b,'输入区',cx,565,1440-cx,295,'#ffffff',0)
  vals=['目录  未指定','名称  未指定','Coding agent  Codex','机器  Mac Studio','父机组  PendingCrew'] if s=='新建机组' else ['目录  crewground','名称  京都','Coding agent  Codex','机器  Mac Studio','父机组  PendingCrew']
  for i,v in enumerate(vals):btn(b,v,cx+22+(i%2)*390,581+(i//2)*42,374,'填写机组')
  txt(b,'发消息以新建机组' if s=='新建机组' else '开始整理这次的设计方向。',cx+24,733,640,16,'#8d998f');btn(b,'发送',1315,807,100,'已创建机组',True)
 elif s in ['人待办','待办完成','编辑待办','待办分组','AI待办','AI待办详情']:
  ai=s in ['AI待办','AI待办详情'];txt(b,'AI 待办' if ai else '人待办',cx+24,171,540,23,bold=True);btn(b,'分组 ▾',1275,168,140,'待办分组')
  for i,(n,sub) in enumerate(([('整理重连验证记录','后端 · 进行中 · 首版验收'),('对照 UI 完成原型','设计 · 等待安排 · 前端设计')] if ai else [('确认前端布局','你 · 等我决定 · 前端设计'),('检查恢复后的上下文','你 · 未开始 · 首版验收'),('阅读设计说明','你 · 已完成 · 未分组')])):
   row(b,('✓ ' if s=='待办完成' or i==2 else '○ ')+n,sub,cx+24,239+i*110,1440-cx-48,'AI待办详情' if ai else '编辑待办',h=94)
   btn(b,'✓',cx+38,291+i*110,42,'待办完成')
  btn(b,'＋ 添加待办',cx+24,615,180,'编辑待办')
 elif s in ['文档数据','片段编辑','片段保存','关联示例','生成页面','页面预览']:
  txt(b,'文档与数据',cx+24,172,550,24,bold=True);btn(b,'＋ 片段',1282,168,130,'片段编辑')
  row(b,'设计说明','文本片段 · 设计更新 · 关联 2 个示例',cx+24,237,1440-cx-48,'片段编辑',h=94)
  row(b,'界面参数','数据片段 · 版本 3 · 浅白 / 轻玻璃',cx+24,347,1440-cx-48,'片段编辑',h=94)
  btn(b,'关联示例',cx+24,474,160,'关联示例');btn(b,'生成页面预览',cx+200,474,190,'生成页面')
 else:
  txt(b,'计划与进展',cx+24,172,600,23,bold=True)
  row(b,('▸' if s=='进展折叠' else '▾')+' 团队界面设计','设计 · 进行中 · 下一步：与你一起确认原型',cx+24,238,1440-cx-48,'计划进展' if s=='进展折叠' else '进展折叠',h=90)
  if s!='进展折叠':
   row(b,('▾' if s=='步骤展开' else '▸')+' 01 可点击原型','已完成设计稿 · 待人类验收',cx+48,350,1440-cx-72,'计划进展' if s=='步骤展开' else '步骤展开',h=84)
   if s=='步骤展开':txt(b,'✓ 原生文字与图层可编辑\n✓ 状态跳转与返回连线\n○ 由你确认布局与流程',cx+70,454,660,16,h=124);btn(b,'查看设计说明',cx+70,580,190,'文档数据')
   row(b,'▸ 02 产品集成','尚未开始 · 依赖设计确认',cx+48,664,1440-cx-72,'进展折叠',h=84)
 # Native linked state sheets: composited panels, with explicit back connections.
 if s=='成员':modal(b,'本群成员',[ '你 · 人类','机长 · Codex · 等待安排','后端 · Codex · 工作中','设计 · Codex · 等待消息'],[('查看后端','成员详情'),('返回群聊','群聊')])
 if s=='成员详情':modal(b,'后端 · PendingCrew',['Coding agent：Codex','工作：整理验证记录','当前会话：保留上下文'],[('发消息','单聊'),('返回成员','成员')])
 if s in ['消息分类','分类展开']:modal(b,'本群消息分类',['全部消息','▾ 实现讨论' if s=='分类展开' else '▸ 实现讨论','    验证讨论' if s=='分类展开' else '界面讨论','分类引用原消息，不是磁盘目录。'],[('展开分类','分类展开'),('查看分类消息','群聊')])
 if s=='编辑待办':modal(b,'编辑待办',['标题：确认前端布局','负责人：你','分组：前端设计','备注：保留顶部原有 Tabs 与一个搜索入口'],[('保存并完成','待办完成'),('更改分组','待办分组')])
 if s=='待办分组':modal(b,'待办分组',['✓ 前端设计','○ 首版验收','○ 未分组','＋ 新分组：设计反馈'],[('保存分组','人待办'),('返回','back')])
 if s=='AI待办详情':modal(b,'整理重连验证记录',['负责人：后端 · Codex','状态：进行中','关联：文档 / 设计说明','完成结果需依据实际证据确认'],[('查看文档','文档数据'),('返回 AI 待办','AI待办')])
 if s in ['片段编辑','片段保存']:modal(b,'编辑文本片段',['标题：设计说明','内容：个人的可靠私人团队。','浅白与轻玻璃；每个机组保留自己的内容。','已保存版本 4' if s=='片段保存' else '文字为原生可编辑图层'],[('保存片段','片段保存'),('关联示例','关联示例')])
 if s=='关联示例':modal(b,'关联示例',['✓ 群聊 / 顶部 Tabs','✓ 新建机组 / 首条消息','○ 文档 / 页面预览','引用同一片段，不复制出第二份内容'],[('保存关联','文档数据'),('生成预览','生成页面')])
 if s=='生成页面':modal(b,'生成页面预览',['来源：设计说明 + 界面参数','页面名称：团队设计说明','合成数据预览 · 不上传、不公开发布'],[('查看页面','页面预览'),('返回文档','文档数据')])
 if s=='页面预览':modal(b,'团队设计说明',['PendingCrew','个人的可靠私人团队','消息、待办、资料与进展，留在各自机组。','页面预览 · 未发布'],[('编辑来源','片段编辑'),('返回文档','文档数据')])
 if s in ['搜索','全局搜索','搜索结果']:modal(b,'搜索',['⌕  设计','范围：全部机组' if s=='全局搜索' else '范围：当前群 PendingCrew','设计说明 · 文档数据 · PendingCrew','确认前端布局 · 人待办 · PendingCrew'],[('当前群','搜索结果'),('全局','全局搜索'),('打开结果','文档数据')][:2] if s!='搜索结果' else [('打开设计说明','文档数据'),('返回搜索','搜索')])
 if s=='机器菜单':modal(b,'机器',['🖥️ Mac Studio','💻 MacBook Pro','＋ 添加机器','设置'],[('切换机器','机器切换'),('设置','设置')])
 if s=='机器切换':modal(b,'MacBook Pro',['连接未建立','此机器的机组保留独立列表','选择本机可返回当前团队'],[('Mac Studio','群聊'),('连接设置','连接设置')])
 if s in ['设置','连接设置','通知设置']:modal(b,{'设置':'设置','连接设置':'设置 · 机器与连接','通知设置':'设置 · 通知'}[s],{'设置':['机器与连接','通知','机器状态在设置中管理','设计原型 · 所有连接与凭据均为合成示例'],'连接设置':['🖥️ Mac Studio · 在线','💻 MacBook Pro · 离线','连接地址：示例本机','状态：在线 / 暂停接收'],'通知设置':['✓ 提及我时通知','✓ 待办等待我决定时通知','○ 普通消息通知','安静时间：22:00 — 08:00']}[s],[('机器与连接','连接设置'),('通知','通知设置')])
 if s=='机组详情':modal(b,'PendingCrew · 机组详情',['名称：PendingCrew','目录：PendingCrew','父机组：PendingName','从侧栏机组上下文打开；点击热点模拟右键'],[('Codex 会话','Codex控制'),('返回群聊','群聊')])
 if s=='Codex控制':modal(b,'Codex 会话',['会话：已暂停 · 上下文保留','模型：GPT-6 · 推理强度：High','重开不会自动执行，恢复需要手动确认。'],[('手动恢复','恢复确认'),('模型选项','模型选择')])
 if s=='模型选择':modal(b,'模型选项',['✓ GPT-6 · High','○ GPT-6 · Medium','○ GPT-6.1 · High','仅设计选项，不真实请求模型。'],[('应用选择','Codex控制'),('返回','back')])
 if s=='恢复确认':modal(b,'手动恢复会话',['将使用已有上下文继续。','先核对当前机组与未完成工作。','本画板仅模拟恢复后的界面。'],[('确认恢复','已恢复'),('取消','Codex控制')])
 if s=='已恢复':modal(b,'会话已恢复',['状态：工作中（模拟）','记录：由你显式恢复','可发新消息，或叫停当前工作。'],[('叫停工','停工确认'),('回到消息','群聊')])
 if s=='停工确认':modal(b,'叫停当前工作？',['通知会话停止当前工作并保留进度。','需要等待安全点，不会自动重开任务。'],[('确认叫停工','已停工'),('取消','已恢复')])
 if s=='已停工':modal(b,'当前工作已停止',['进度与上下文已保留（模拟）','状态：已停工','继续需要再次手动恢复。'],[('返回会话','Codex控制'),('退出应用','退出询问')])
 if s=='退出询问':modal(b,'退出 PendingCrew？',['当前有 1 个会话工作中（模拟）','直接退出：立即关闭应用','安全暂停：保存上下文后退出','叫停工：等待工作结束后退出'],[('直接退出','空白'),('安全暂停','安全暂停')]);btn(b,'叫停工并退出',1010,542,230,'等待停工')
 if s=='安全暂停':modal(b,'安全暂停完成',['上下文与恢复点已保留（模拟）','下次打开不会自动执行。'],[('退出','空白'),('模拟重新打开','重新打开')])
 if s=='等待停工':modal(b,'正在等待安全停工',['后端正在整理当前进度（模拟）','你可以关闭这个等待窗口。','强制关闭可能丢失本次尚未保存的上下文。'],[('关闭等待窗口','关闭等待窗'),('强制关闭…','强制关闭')])
 if s=='关闭等待窗':modal(b,'等待窗口已关闭',['停工请求仍保留（模拟）','应用保持打开，尚未执行退出。'],[('查看停工状态','已停工'),('再次退出','退出询问')])
 if s=='强制关闭':modal(b,'强制关闭？',['当前工作可能尚未保存。','这是退出模拟，不触碰真实进程或数据。'],[('强制关闭','空白'),('继续等待','等待停工')])

# Reuse the original native document and browser companion serializer.
old=(P.parent/'2026-10-13-folders-chat/build.py').read_text()
tail=old[old.index('page=default('):]
tail=tail.replace('PendingCrew-FoldersChat.sketch','PendingCrew-PrivateTeam.sketch').replace("state=='群聊'","state=='群聊'").replace('微信式布局 · 独立文件夹与结构化进展','PendingCrew · 私人团队 · 2026-10-24').replace("'start':'群聊'","'start':'群聊'")
# b['layers'] are native shapes/text and native FlowConnection objects.
exec(tail)

# A native Symbols page is included for continued design in Sketch.
component_specs=[('控件 / 主要按钮',160,40,'发送','#dceee1'),('控件 / 顶部 Tab',110,40,'消息','#edf2ee'),('控件 / 元数据字段',374,34,'目录  未指定','#f0f3f1'),('消息 / 接收气泡',540,66,'收到，先保留现场。','#ffffff'),('机器 / 浅圆 Emoji',44,44,'🖥️','#f8faf6'),('空态 / 玻璃加号',94,94,'＋','#e8eee7')]
components=[]
for i,(n,w,h,t,c) in enumerate(component_specs):
 master=base('SymbolMaster',n,i%3*600,i//3*170,w,h);master.update(allowsOverrides=True,includeBackgroundColorInInstance=False,hasBackgroundColor=False,isFlowHome=False,symbolID=uid())
 master['layers']=[rect(n+' 背景',0,0,w,h,c,min(h/2,16)),text(t,12,8,w-24,18 if h<80 else 50,'#31483a',h-10)]
 components.append(master)
comp_page=default(json.loads((P/'schema-source/package/dist/page.schema.json').read_text()));comp_page.update(name='可复用组件 · Symbols',isVisible=True,booleanOperation=-1,resizingConstraint=63,layers=components)
# Use an actual symbol instance for the empty-state control, with a native prototype link.
empty=boards['空白'];empty['layers']=empty['layers'][:-2]
instance=base('SymbolInstance','空态 / 玻璃加号 实例',955,386,94,94);instance.update(symbolID=components[-1]['symbolID'],scale=1);instance['flow']={'_class':'MSImmutableFlowConnection','destinationArtboardID':boards['新建机组']['do_objectID'],'animationType':0};empty['layers'].append(instance)
doc['pages'].append({'_class':'MSJSONFileReference','_ref_class':'MSImmutablePage','_ref':'pages/'+comp_page['do_objectID']})
meta['pagesAndArtboards'][comp_page['do_objectID']]={'name':comp_page['name'],'artboards':{m['do_objectID']:{'name':m['name']} for m in components}}
with zipfile.ZipFile(P/'PendingCrew-PrivateTeam.sketch','w',zipfile.ZIP_DEFLATED) as z:
 for n,v in [('document.json',doc),('meta.json',meta),('user.json',obj('User')),('pages/'+page['do_objectID']+'.json',page),('pages/'+comp_page['do_objectID']+'.json',comp_page)]:z.writestr(n,json.dumps(v,ensure_ascii=False))
