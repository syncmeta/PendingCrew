import json, uuid, pathlib, zipfile, html
P=pathlib.Path(__file__).parent
schema=json.loads((P/'schema-source/package/dist/file-format.schema.json').read_text())
D=schema['definitions']
def uid(): return str(uuid.uuid4()).upper()
def default(s):
 if '$ref' in s:
  n=s['$ref'].split('/')[-1]
  if n=='Uuid': return uid()
  if n=='PointString': return '{0, 0}'
  if n=='PointListString': return '{{0, 0}, {0, 0}}'
  return default(D[n])
 if 'const' in s: return s['const']
 if 'enum' in s: return s['enum'][0]
 if 'oneOf' in s: return default(s['oneOf'][0])
 t=s.get('type')
 if t=='object': return {k:default(s['properties'][k]) for k in s.get('required',[])}
 if t=='array': return []
 if t=='boolean': return False
 if t in ['integer','number']: return s.get('minimum',0)
 return ''
def obj(n): return default(D[n])
def color(hex):
 c=obj('Color');c.update(alpha=1,red=int(hex[1:3],16)/255,green=int(hex[3:5],16)/255,blue=int(hex[5:7],16)/255);return c
def frame(x,y,w,h):
 r=obj('Rect');r.update(x=x,y=y,width=w,height=h);return r
def base(n,name,x,y,w,h):
 a=obj(n);a.update(name=name,isVisible=True,resizingConstraint=63,booleanOperation=-1,frame=frame(x,y,w,h));return a
def rect(name,x,y,w,h,fill='#ffffff',radius=0):
 a=base('Rectangle',name,x,y,w,h);a.update(isClosed=True,fixedRadius=radius,hasConvertedToNewRoundCorners=True)
 for pt in ['{0, 0}','{1, 0}','{1, 1}','{0, 1}']:
  c=obj('CurvePoint');c.update(point=pt,curveFrom=pt,curveTo=pt,cornerRadius=radius);a['points'].append(c)
 st=obj('Style');f=obj('Fill');f.update(isEnabled=True,fillType=0,color=color(fill));st['fills']=[f];a['style']=st;return a
def text(label,x,y,w=700,size=16,fill='#263238',h=40):
 a=base('Text',label,x,y,w,h);a['textBehaviour']=1
 st=obj('StringAttribute');st.update(location=0,length=len(label.encode('utf-16-le'))//2)
 font=obj('FontDescriptor');font['attributes'].update(name='PingFangSC-Regular',size=size)
 st['attributes']={'MSAttributedStringFontAttribute':font,'MSAttributedStringColorAttribute':color(fill)}
 a['attributedString'].update(string=label,attributes=[st]);return a
names=['01 总览','02 机组与成员','03 会话','04 工作','05 计划详情','06 备注编辑','07 备注已保存','08 待我处理','09 答复问题','10 答复已提交','11 稍后处理','12 撤回确认','13 已撤回','14 恢复提示','15 已恢复','16 暂不恢复','17 方案','18 冻结工作']
boards={n:base('Artboard',n,(i%4)*1360,(i//4)*880,1280,800) for i,n in enumerate(names)}
links=[]
def add(b,a): b['layers'].append(a);return a
def button(b,label,x,y,w=170,target=None,primary=False):
 a=add(b,rect(label,x,y,w,44,'#e2edeb' if primary else '#eef1f2',8));add(b,text(label,x+14,y+11,w-20,15,h=26))
 if target:
  a['flow']={'_class':'MSImmutableFlowConnection','destinationArtboardID':boards[target]['do_objectID'],'animationType':0};links.append((b['name'],label,target))
def row(b,label,y,target=None,sub=''):
 a=add(b,rect(label,282,y,954,76,'#f3f5f5',8));add(b,text(label,302,y+13,880,17));
 if sub:add(b,text(sub,302,y+43,880,13,'#667578',25))
 if target:a['flow']={'_class':'MSImmutableFlowConnection','destinationArtboardID':boards[target]['do_objectID'],'animationType':0};links.append((b['name'],label,target))
nav=[('总览',names[0]),('机组与成员',names[1]),('会话',names[2]),('工作',names[3]),('待我处理 · 3',names[7]),('方案',names[16])]
for n,b in boards.items():
 b.update(hasBackgroundColor=True,includeBackgroundColorInExport=True,backgroundColor=color('#ffffff'),isFlowHome=n==names[0])
 add(b,rect('画板背景',0,0,1280,800,'#ffffff'));add(b,rect('侧边栏',0,0,248,800,'#f4f6f6'));add(b,text('PendingCrew',24,32,208,24));add(b,text('我的私人团队',24,77,210,14,'#667578'))
 for i,(label,target) in enumerate(nav):button(b,label,18,146+i*58,210,target)
 add(b,text('本机 · Codex 优先',24,674,212,14,'#667578'));add(b,text('设计草图 / 合成数据',24,713,212,13,'#667578'))
 add(b,text(n[3:],282,32,750,28));add(b,text('原型 / 规划 · 用来一起设计新版界面',282,77,880,14,'#667578'))
b=boards[names[0]]
add(b,text('早上好。团队有 3 件事需要你。',282,138,900,26));button(b,'查看待我处理',282,197,210,names[7],True)
row(b,'PendingCrew 开发',277,names[1],'2 位 Codex 成员 · 正在推进工作 · 1 个待答复')
row(b,'W12 · 工作视图与检查点',370,names[4],'负责人：后端 · 进行中 · 下一步：接 agent 工具')
row(b,'后台重启，2 个会话等待恢复',463,names[13],'由你决定是否恢复；不会自动重放消息')
b=boards[names[1]];row(b,'PendingCrew 开发 · 机长',145,names[2],'Codex · 等待输入');row(b,'后端 · 工作成员',238,names[2],'Codex · 进行中 · W12');row(b,'设计 · 工作成员',331,names[3],'原型中的角色 · 目前由你设计');add(b,text('Claude 接入 / 跨厂商协作：规划位置，当前不显示为在线。',282,475,930,16))
b=boards[names[2]];add(b,text('PendingCrew 开发 / 后端',282,130,900,21));row(b,'你：先把新版 Codex 用起来。',181);row(b,'后端：工作操作接口已接上，接下来验证 agent 工具。',274,names[4],'关联 W12 · 合成消息');row(b,'机长：界面设计交给你，后端继续推进。',367);add(b,rect('消息输入框',282,642,740,62,'#f3f5f5',8));add(b,text('给团队发消息…',301,661,690,16,'#667578'));button(b,'模拟发送',1040,651,194,names[2]);button(b,'停止会话',282,557,160,names[13])
b=boards[names[3]];row(b,'#11 · 尽快用上新版 PendingCrew',142,names[4],'原始需求原文保留 · 关联 W12、W13');row(b,'W12 · 工作视图与检查点',235,names[4],'进行中 · 后端负责 · 完成不强制附凭据');row(b,'W13 · 前端交互设计',328,names[4],'由你设计 · 备注和状态只保留一份');add(b,text('统一编号跨机组可定位；最多 3 层 / 可读代号 / 旧别名：规划。',282,468,940,16));button(b,'查看冻结工作',282,542,210,names[17])
for n in names[4:7]:
 b=boards[n];add(b,text('W12 · 工作视图与检查点',282,137,900,25));add(b,text('源需求 #11   /   负责人 后端   /   进行中',282,184,880,16));row(b,'目标：让你看清团队做了什么、下一步是什么。',231);row(b,'验收：统一工作视图，恢复后不重复消息。',324);add(b,text('备注',282,442,600,18));add(b,rect('备注内容',282,477,952,95,'#f3f5f5',8));add(b,text('先验证 Codex 首版，再用新版开发新版。' if n!=names[6] else '已保存：先让工作视图保持简洁，详细记录按需展开。',301,498,900,17));button(b,'编辑备注' if n==names[4] else '保存备注',282,608,170,names[5] if n==names[4] else names[6],True);button(b,'返回工作',471,608,170,names[3])
b=boards[names[7]];row(b,'H18 · 首版恢复后是否继续 W12？',142,names[8],'PendingCrew 开发 · 对应 W12 · 只在这里处理一次');row(b,'H19 · 需要你选择界面布局',235,names[8],'设计问题 · 合成数据');row(b,'H20 · 候选包是否切换日常数据？',328,names[8],'需要明确决定，原型不会执行任何真实切换')
for n in names[8:13]:
 b=boards[n];add(b,text('H18 · 首版恢复后是否继续 W12？',282,143,910,25));add(b,text('来自后端 · PendingCrew 开发 · 关联 W12',282,191,900,16))
 add(b,text('已提交：继续推进，先完成工具验收。' if n==names[9] else '稍后处理，问题仍保留。' if n==names[10] else '已撤回，团队会看到撤回结果。' if n==names[12] else '问题和答复均为原型合成内容。',282,258,900,20))
 if n==names[8]:
  add(b,rect('答复输入框',282,315,950,118,'#f3f5f5',8));add(b,text('继续推进，先完成工具验收。',302,337,900,17));button(b,'提交答复',282,473,170,names[9],True);button(b,'稍后',471,473,140,names[10]);button(b,'撤回问题',630,473,170,names[11])
 elif n==names[11]:button(b,'确认撤回',282,350,180,names[12],True);button(b,'取消',482,350,150,names[8])
 button(b,'返回待我处理',282,594,190,names[7])
for n in names[13:16]:
 b=boards[n];add(b,text('后台已重启',282,147,930,27));row(b,'机长 · 原会话可恢复',219,sub='原会话身份保持；不会自动重放旧消息');row(b,'后端 · 原会话可恢复',310,sub='相关工作 W12 仍在原工作视图中')
 add(b,text('2 个会话已恢复，等待新的输入。' if n==names[14] else '暂不恢复，工作记录保留。' if n==names[15] else '选择恢复后接回原会话。你也可以稍后再决定。',282,438,900,18));button(b,'恢复原会话',282,511,190,names[14],True);button(b,'稍后再说',491,511,170,names[15]);button(b,'回到总览',680,511,170,names[0])
b=boards[names[16]];row(b,'记忆：个人偏好与团队知识',142,sub='仅方案 · 让团队了解你 · 尚未实施');row(b,'Jev：多种决策后端',235,sub='仅方案 · 讨论后再开发');row(b,'跨厂商成员：统一协作边界',328,sub='通用机组与消息框架 · 不依赖 Codex 内部通信');add(b,text('这些是设计讨论的入口，不是已启用的产品功能。',282,477,930,18))
b=boards[names[17]];add(b,text('W8 · 旧机组中的工作',282,142,900,25));row(b,'只读：所属机组已移除',222,sub='仍可查看记录，不能继续更新或推进');row(b,'最后进展：候选构建完成',315,sub='历史记录保留；默认不列入“我的工作”');button(b,'返回工作',282,463,180,names[3])
page=default(json.loads((P/'schema-source/package/dist/page.schema.json').read_text()));page.update(name='PendingCrew · 私人团队草图',isVisible=True,booleanOperation=-1,resizingConstraint=63,layers=list(boards.values()))
doc=default(schema['properties']['document']);doc['pages']=[page]
meta=obj('Meta');meta.update(version=146,appVersion='2026.3',build=233959,commit='prototype',pagesAndArtboards={page['do_objectID']:{'name':page['name'],'artboards':{b['do_objectID']:{'name':n} for n,b in boards.items()}}});meta['created'].update(version=146,appVersion='2026.3',build=233959)
user=obj('User')
# Official schema validation of the actual in-memory document and every layer.
def validate(v,s,path='root'):
 if '$ref' in s:return validate(v,D[s['$ref'].split('/')[-1]],path)
 if 'const' in s:assert v==s['const'],(path,'const')
 if 'enum' in s:assert v in s['enum'],(path,'enum',v)
 if 'oneOf' in s:
  good=0
  for o in s['oneOf']:
   try:validate(v,o,path);good+=1
   except (AssertionError,KeyError,TypeError):pass
  assert good==1,(path,'oneOf',good);return
 t=s.get('type')
 if t=='object':
  assert isinstance(v,dict),(path,t)
  for k in s.get('required',[]):assert k in v,(path,'missing',k)
  for k,val in v.items():
   if k in s.get('properties',{}):validate(val,s['properties'][k],path+'.'+k)
   elif s.get('additionalProperties') is False and not s.get('patternProperties'):raise AssertionError((path,'extra',k))
   elif s.get('patternProperties'):
    for sub in s['patternProperties'].values():validate(val,sub,path+'.'+k)
 elif t=='array':
  assert isinstance(v,list),(path,t)
  for i,a in enumerate(v):validate(a,s['items'],path+f'[{i}]')
 elif t=='string':
  assert isinstance(v,str),(path,t)
  if s.get('format')=='uuid':uuid.UUID(v)
 elif t=='boolean':assert isinstance(v,bool),(path,t)
 elif t in ('number','integer'):
  assert isinstance(v,(int,float)) and not isinstance(v,bool),(path,t)
  assert v>=s.get('minimum',float('-inf')) and v<=s.get('maximum',float('inf')),(path,'bounds',v)
validate(doc,schema['properties']['document']);validate(meta,D['Meta']);validate(user,D['User'])
document_file=dict(doc);document_file['pages']=[{'_class':'MSJSONFileReference','_ref_class':'MSImmutablePage','_ref':'pages/'+page['do_objectID']}]
with zipfile.ZipFile(P/'PendingCrew-Private-Team.sketch','w',zipfile.ZIP_DEFLATED) as z:
 for name,value in [('document.json',document_file),('meta.json',meta),('user.json',user),('pages/'+page['do_objectID']+'.json',page)]:z.writestr(name,json.dumps(value,ensure_ascii=False))
(P/'prototype-map.json').write_text(json.dumps({'artboards':len(boards),'links':links,'start':names[0]},ensure_ascii=False,indent=2))
(P/'editable-document.json').write_text(json.dumps(doc,ensure_ascii=False))
print('Created',len(boards),'artboards,',len(links),'native links; official schema constraints checked')
# Companion browser preview uses the same editable layer geometry and native flow graph.
panels=[]
for n,b in boards.items():
 body=[]
 for a in b['layers']:
  if 'flow' in a:
   f=a['frame'];target=next(k for k,v in boards.items() if v['do_objectID']==a['flow']['destinationArtboardID'])
   body.append('<button class="hotspot" aria-label="'+html.escape(a['name'],quote=True)+'" data-target="'+html.escape(target,quote=True)+'" style="left:'+str(f['x']/12.8)+'%;top:'+str(f['y']/8)+'%;width:'+str(f['width']/12.8)+'%;height:'+str(f['height']/8)+'%"></button>')
 panels.append('<section data-board="'+n+'" hidden><img alt="'+n+'" src="exports/'+n+'.png">'+''.join(body)+'</section>')
preview='''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>PendingCrew 原型预览</title><style>body{margin:0;background:#dde2e3;font:14px system-ui}header{display:flex;gap:16px;padding:14px 24px;align-items:center}button,select{font:inherit;padding:8px 12px}main{max-width:1280px;margin:auto}section{position:relative}img{display:block;width:100%}.hotspot{position:absolute;border:0;background:transparent;cursor:pointer;padding:0}.hotspot:hover,.hotspot:focus-visible{background:#00695c18;outline:2px solid #487d73}section[hidden]{display:none}small{color:#4f6265}</style><header><strong>PendingCrew · 可交互草图</strong><button id="back">返回</button><select id="jump" aria-label="画板"></select><small>原型合成数据；文本编辑在 Sketch 中。所有跳转均来自原生 Sketch 连接。</small></header><main>'''+''.join(panels)+'''</main><script>const sections=[...document.querySelectorAll('section')],jump=document.querySelector('#jump'),history=[];for(const s of sections){const o=document.createElement('option');o.textContent=s.dataset.board;jump.append(o)}function show(n,push=true){const old=sections.find(s=>!s.hidden);if(push&&old)history.push(old.dataset.board);for(const s of sections)s.hidden=s.dataset.board!==n;jump.value=n}document.addEventListener('click',e=>{if(e.target.dataset.target)show(e.target.dataset.target)});jump.addEventListener('change',()=>show(jump.value));document.querySelector('#back').onclick=()=>{const n=history.pop();if(n)show(n,false)};show(sections[0].dataset.board,false);</script></html>'''
(P/'index.html').write_text(preview)
