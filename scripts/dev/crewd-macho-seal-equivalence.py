#!/usr/bin/env python3
"""Read-only Mach-O code equivalence across signing, never signs/strips/executes.
Only signature command/blob, its zero alignment padding and mathematically bound
__LINKEDIT signature-size/page rounding may change. Every other byte is compared.
"""
import hashlib,json,pathlib,struct

def fail(message):raise ValueError('Mach-O seal equivalence: '+message)
def digest(data):return hashlib.sha256(data).hexdigest()
def slices(data):
 if data[:4] in [bytes.fromhex('cffaedfe'),bytes.fromhex('feedfacf')]:return [({'cpu':None,'subtype':None,'alignment':None},data)]
 if len(data)<8:fail('truncated container header')
 magic=data[:4];formats={bytes.fromhex('cafebabe'):('>',False),bytes.fromhex('bebafeca'):('<',False),bytes.fromhex('cafebabf'):('>',True),bytes.fromhex('bfbafeca'):('<',True)}
 if magic not in formats:fail('unsupported executable container')
 endian,wide=formats[magic];count=struct.unpack_from(endian+'I',data,4)[0];size=32 if wide else 20
 if not 1<=count<=16 or len(data)<8+count*size:fail('invalid fat table')
 rows=[];ranges=[];covered=8+count*size
 for index in range(count):
  fields=struct.unpack_from(endian+('IIQQII' if wide else 'IIIII'),data,8+index*size);cpu,subtype,offset,length,alignment=fields[:5]
  if alignment>30 or offset<covered or offset% (1<<alignment) or not length or offset+length>len(data) or (wide and fields[5]!=0):fail('invalid fat slice bounds')
  if any(offset<end and offset+length>start for start,end in ranges):fail('overlapping fat slices')
  ranges.append((offset,offset+length));rows.append(({'cpu':cpu,'subtype':subtype,'alignment':alignment,'containerMagic':magic.hex()},data[offset:offset+length]))
 cursor=covered
 for start,end in sorted(ranges):
  if any(data[cursor:start]):fail('nonzero fat alignment data would escape code comparison')
  cursor=end
 if any(data[cursor:]):fail('nonzero fat trailing data')
 return rows

def thin(data):
 endian='<' if data[:4]==bytes.fromhex('cffaedfe') else '>' if data[:4]==bytes.fromhex('feedfacf') else None
 if endian is None or len(data)<32:fail('only explicit 64-bit Mach-O supported')
 header=list(struct.unpack_from(endian+'8I',data));cpu=header[1]
 if cpu not in [0x100000c,0x1000007] or header[3] not in [2,6,8]:fail('unsupported CPU/file type')
 ncmds,total=header[4:6];end=32+total
 if not 0<ncmds<=4096 or end>len(data):fail('invalid load command header')
 cursor=32;commands=[];signature=None;linkedit=None;sections=[]
 for _ in range(ncmds):
  if cursor+8>end:fail('truncated load command')
  cmd,size=struct.unpack_from(endian+'2I',data,cursor)
  if size<8 or size%8 or cursor+size>end:fail('invalid load command length')
  raw=bytearray(data[cursor:cursor+size])
  if cmd==0x1d:
   if size!=16 or signature is not None:fail('duplicate/invalid signature command')
   offset,length=struct.unpack_from(endian+'2I',raw,8);signature=(offset,length)
  else:
   if cmd==0x19:
    if size<72:fail('truncated segment')
    name=bytes(raw[8:24]).rstrip(b'\0');vmaddr,vmsize,fileoff,filesize=struct.unpack_from(endian+'4Q',raw,24);nsects=struct.unpack_from(endian+'I',raw,64)[0]
    if size!=72+80*nsects:fail('invalid segment section table')
    for index in range(nsects):
     section=72+index*80;section_size=struct.unpack_from(endian+'Q',raw,section+40)[0];section_offset=struct.unpack_from(endian+'I',raw,section+48)[0];flags=struct.unpack_from(endian+'I',raw,section+64)[0]
     if section_offset and (flags&0xff) not in [1,12,18]:sections.append((section_offset,section_size))
    if name==b'__LINKEDIT':
     if linkedit is not None or nsects:fail('invalid linkedit segment')
     linkedit=(vmaddr,vmsize,fileoff,filesize);struct.pack_into(endian+'Q',raw,32,0);struct.pack_into(endian+'Q',raw,48,0)
   commands.append(bytes(raw))
  cursor+=size
 if cursor!=end or linkedit is None:fail('missing/invalid linkedit')
 signed=signature is not None and signature[1]>0
 if signature is not None and not signed and signature!=(0,0):fail('ambiguous empty signature range')
 content_end=signature[0] if signed else len(data)
 if content_end<end or linkedit[2]>content_end or linkedit[2]+linkedit[3]!=len(data):fail('signature/linkedit extent mismatch')
 # Universal x86_64 vendor binaries may retain the 16KiB linker convention;
 # bind the post-seal size to the same possible original 4KiB/16KiB policy.
 pages=[16384] if cpu==0x100000c else [4096,16384]
 rounding={page for page in pages if linkedit[1]==(linkedit[3]+page-1)//page*page}
 if not rounding:fail('linkedit VM size is not exact signature-bound page rounding')
 if signed:
  offset,length=signature
  if offset+length!=len(data) or length<12:fail('signature must be the complete terminal range')
  magic,blob_length=struct.unpack_from('>2I',data,offset)
  if magic!=0xfade0cc0 or not 12<=blob_length<=length:fail('invalid signature superblob extent')
 if any(offset<end or offset+size>content_end for offset,size in sections):fail('signature/commands overlap actual section data')
 header[4]=len(commands);header[5]=sum(map(len,commands))
 return {'cpu':cpu,'subtype':header[2],'header':struct.pack(endian+'8I',*header),'commands':commands,'commandEnd':end,'contentEnd':content_end,'signed':signed,'rounding':rounding,'sections':sections,'data':data}

def equivalent_bytes(before,after):
 pre=slices(before);post=slices(after)
 if len(pre)!=len(post) or [metadata for metadata,_ in pre]!=[metadata for metadata,_ in post]:fail('fat architecture identity/order changed')
 results=[]
 for (metadata,a),(_,b) in zip(pre,post):
  x=thin(a);y=thin(b)
  if metadata['cpu'] is not None and (metadata['cpu']!=x['cpu'] or metadata['subtype']!=x['subtype']):fail('fat header differs from slice identity')
  if not x['rounding'].intersection(y['rounding']):fail('linkedit page-rounding policy changed')
  if not y['signed'] or x['header']!=y['header'] or x['commands']!=y['commands']:fail('non-signature header/load command changed')
  start=max(x['commandEnd'],y['commandEnd'])
  if any(a[x['commandEnd']:start]) or any(b[y['commandEnd']:start]):fail('added signature command overwrote nonzero header padding')
  if any(offset<start for offset,size in x['sections']+y['sections']):fail('signature command grew into executable section')
  finish=x['contentEnd']
  if y['contentEnd']<finish or (x['signed'] and y['contentEnd']!=finish) or y['contentEnd']-finish>15 or any(b[finish:y['contentEnd']]):fail('unsigned payload/signature boundary changed')
  if a[start:finish]!=b[start:finish]:fail('executable/loaded/linkedit payload bytes changed')
  canonical=x['header']+b''.join(x['commands'])+a[start:finish]
  results.append({'architecture':metadata,'cpu':x['cpu'],'subtype':x['subtype'],'unsignedCodeSHA256':digest(canonical),'payloadBytesCompared':finish-start,'preSignaturePresent':x['signed'],'postSignaturePresent':True,'onlySignatureAndBoundedLinkeditMetadataChanged':True})
 return {'preSHA256':digest(before),'postSHA256':digest(after),'slices':results,'codeEquivalent':True,'actualSignatureValidityProven':False}

def equivalent_files(before,after):return equivalent_bytes(pathlib.Path(before).read_bytes(),pathlib.Path(after).read_bytes())
