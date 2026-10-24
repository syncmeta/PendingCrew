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
 a=base('ShapePath',name,x,y,w,h);a.update(isClosed=True,edited=True)
 import math
 r=min(radius,w/2,h/2)
 pts=[]
 if r:
  for cx,cy,start in [(w-r,r,-90),(w-r,h-r,0),(r,h-r,90),(r,r,180)]:
   for i in range(9):
    theta=math.radians(start+i*90/8);pts.append((cx+r*math.cos(theta),cy+r*math.sin(theta)))
 else:pts=[(0,0),(w,0),(w,h),(0,h)]
 for px,py in pts:
  p='{'+str(px/w)+', '+str(py/h)+'}'
  c=obj('CurvePoint');c.update(point=p,curveFrom=p,curveTo=p,curveMode=1);a['points'].append(c)
 st=obj('Style');f=obj('Fill');f.update(isEnabled=True,fillType=0,color=color(fill));st['fills']=[f];a['style']=st;return a
def text(label,x,y,w=700,size=16,fill='#263238',h=40):
 a=base('Text',label,x,y,w,h);a['textBehaviour']=1
 st=obj('StringAttribute');st.update(location=0,length=len(label.encode('utf-16-le'))//2)
 font=obj('FontDescriptor');font['attributes'].update(name='PingFangSC-Regular',size=size)
 st['attributes']={'MSAttributedStringFontAttribute':font,'MSAttributedStringColorAttribute':color(fill)}
 a['attributedString'].update(string=label,attributes=[st]);return a
