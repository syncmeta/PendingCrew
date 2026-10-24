"""Add the accepted pre-rewrite right work area to the delivered native graph.

Reads the existing Runtime-Environment document; never changes it or its
generator. No GUI, App build, new dependency or network operation.
"""
from refined_helpers import *
from copy import deepcopy
import ast, hashlib

SOURCE = P / 'PendingCrew-SwiftUI-Runtime-Environment.sketch'
OUTPUT = P / 'PendingCrew-SwiftUI-Original-Right-Layout.sketch'
SOURCE_SHA = '646b7d41046af2cfadd6757859e3160bf060dc7727c6e668dd9757f9094cf8b3'
assert hashlib.sha256(SOURCE.read_bytes()).hexdigest() == SOURCE_SHA
archive = zipfile.ZipFile(SOURCE)
payload = {n: archive.read(n) for n in archive.namelist()}
doc = json.loads(payload['document.json'])
pagekey, compkey = [x['_ref'] + '.json' for x in doc['pages']]
page, comp = json.loads(payload[pagekey]), json.loads(payload[compkey])
original = deepcopy(page)
boards = {b['name'][3:]: b for b in page['layers']}
state_by_number = {int(b['name'][:2]): b for b in page['layers']}
masters = {m['name']: m for m in comp['layers']}
old_master_ids = {m['do_objectID'] for m in comp['layers']}
flows = []
INK, MUTED, LINE, ACCENT, MINT = '#202124', '#858585', '#e7e7e7', '#044735', '#e3eeea'

# Reuse only pure drawing functions. Do not import/execute the old generator.
names = {'put', 'label', 'shape', 'link', 'hotspot', 'path', 'ellipse',
         'icon_raw', 'master', 'instance', 'symbol_icon', 'glass', 'avatar'}
tree = ast.parse((P / 'refine_sidebar.py').read_text())
pure = ast.Module(body=[n for n in tree.body if isinstance(n, ast.FunctionDef)
                       and n.name in names], type_ignores=[])
exec(compile(pure, 'refine_sidebar.py:pure-drawing-only', 'exec'))
PROOF = False

def group(name):
    g = base('Group', name, 0, 0, 1280, 820)
    g['layers'] = []
    return g

def button(b, title, x, y, width, target, primary=False):
    shape(b, 'original-right/button/' + title, x, y, width, 28,
          ACCENT if primary else '#f3f3f3', 7)
    label(b, title, x + 10, y + 6, width - 18, 12,
          '#ffffff' if primary else '#626262', h=19)
    hotspot(b, 'original-right/action/' + title, x, y, width, 28, target)

def composer(b, x, y, width, text_value, target=None, codex=False):
    label(b, '草稿仅本机保存', x + 12, y - 23, max(0, width - 82), 10, MUTED, h=18)
    symbol_icon(b, 'doc', x + width - 49, y - 21, 13, MUTED)
    label(b, '草稿', x + width - 31, y - 24, 31, 10, MUTED, h=20)
    left = x + 12
    if not codex:
        glass(b, 'original-right/composer/todo', left, y, 30, 30, 15)
        symbol_icon(b, 'checklist', left + 7, y + 7, 16, MUTED)
        left += 40
    w = x + width - 12 - left
    glass(b, 'original-right/composer/input', left, y, w, 38, 19)
    label(b, text_value, left + 13, y + 10, w - 59, 12,
          '#a8a8a8' if text_value.startswith('写') else INK, h=20)
    ellipse(b, 'original-right/composer/send', left + w - 33, y + 5, 28, 28,
            ACCENT if target else '#edf1ee', True)
    symbol_icon(b, 'up', left + w - 27, y + 11, 16,
                '#ffffff' if target else '#c4cbc6')
    # Static prototype does not pretend to accept free-form input or send.
    if target:
        hotspot(b, 'original-right/explicit-send-confirm', left + w - 36, y, 36, 38, target)

def roster_master():
    def draw(m):
        label(m, '未观察到运行中的机长', 14, 9, 370, 11, MUTED, h=22)
        label(m, '›  执行器准备', 14, 37, 370, 12, INK, h=24)
        label(m, '待办工作', 14, 86, 90, 16, INK, h=26, weight='semibold')
        shape(m, 'original-right/agent pill', 111, 87, 66, 23, '#f2f2f2', 12)
        label(m, 'Agent 的', 119, 91, 64, 10, MUTED, h=18)
        shape(m, 'original-right/human pill', 181, 87, 60, 23, MINT, 12)
        label(m, '人类的', 190, 91, 52, 10, ACCENT, h=18)
        shape(m, 'original-right/open work', 320, 85, 66, 27, '#f3f3f3', 7)
        label(m, '放大看', 332, 91, 53, 11, ACCENT, h=19)
        label(m, '暂无待处理的人工待办', 14, 129, 370, 12, MUTED, h=25)
        shape(m, 'original-right/vertical split', 0, 350, 400, 1, LINE)
        label(m, '本群成员 4', 14, 366, 360, 11, MUTED, h=24, weight='semibold')
        for i, (name, status, kind) in enumerate([
                ('你', '人类', 'machine'),
                ('机长', '等待安排 · 原会话', 'captain'),
                ('后端', '已停止 · 保留原会话', 'backend'),
                ('设计', '等待消息 · 原会话', 'design')]):
            yy = 425 + i * 74
            avatar(m, kind, 18, yy, 34)
            label(m, name, 67, yy + 1, 290, 13, INK, h=23, weight='medium')
            label(m, status, 67, yy + 28, 295, 11, MUTED, h=22)
            if i: symbol_icon(m, 'chevron.right', 366, yy + 9, 13, MUTED)
            shape(m, 'original-right/member separator', 67, yy + 65, 317, .5, LINE)
    return master('OriginalRight/Original Todo and Members', 400, 766, draw)

def terminal_master():
    def draw(m):
        shape(m, 'original-right/rail', 0, 0, 64, 766, '#f2f1ee')
        shape(m, 'original-right/rail divider', 64, 0, 1, 766, LINE)
        symbol_icon(m, 'chevron.left', 24, 17, 15, MUTED)
        avatar(m, 'captain', 15, 77, 34)
        shape(m, 'original-right/rail selection', 7, 139, 50, 54, '#e2e9e5', 12)
        avatar(m, 'backend', 15, 149, 34)
        avatar(m, 'design', 15, 221, 34)
        shape(m, 'original-right/new member slot', 7, 287, 50, 54, '#e9e9e5', 12, '#cdceca')
        symbol_icon(m, 'plus', 23, 305, 18, MUTED)
        label(m, '后端 · 原成员会话', 82, 15, 300, 13, INK, h=25, weight='semibold')
        label(m, 'Codex · 已停止 · 不会自动恢复', 82, 47, 300, 11, MUTED, h=24)
        shape(m, 'original-right/terminal toolbar divider', 65, 84, 335, .5, LINE)
        label(m, '原会话记录', 82, 105, 280, 12, INK, h=23, weight='medium')
        label(m, '已保留这次的上下文与执行记录。\n只选择或查看会话不会启动任务。\n\n后续输入仍属于这个原会话。',
              82, 149, 295, 12, '#626262', h=125)
        label(m, '核对原记录后，再由你明确恢复或发送。', 82, 302, 288, 11, MUTED, h=50)
    return master('OriginalRight/64pt Rail and Original Terminal', 400, 766, draw)

def new_member_master():
    def draw(m):
        shape(m, 'original-right/rail', 0, 0, 64, 766, '#f2f1ee')
        shape(m, 'original-right/rail divider', 64, 0, 1, 766, LINE)
        symbol_icon(m, 'chevron.left', 24, 17, 15, MUTED)
        for i,kind in enumerate(['captain','backend','design']):
            avatar(m,kind,15,77+i*72,34)
        shape(m, 'original-right/new selection', 7, 287, 50, 54, '#e2e9e5', 12)
        symbol_icon(m, 'plus', 23, 305, 18, MUTED)
        label(m, '新成员会话', 82, 24, 290, 17, INK, h=30, weight='semibold')
        label(m, '写下一件具体要完成的事，\n发送后才会启动。', 82, 68, 290, 12, MUTED, h=48)
        label(m, '执行成员', 82, 145, 290, 11, MUTED, h=22)
        label(m, '执行器', 82, 184, 80, 12, INK, h=24)
        shape(m, 'original-right/executor menu', 170, 179, 209, 29, '#f4f4f4', 7)
        label(m, 'Codex  ⌄', 184, 186, 176, 12, ACCENT, h=22)
        label(m, '后台已配置此执行器；登录、额度与实际启动\n仍以原请求及会话状态为准。', 82, 229, 290, 11, MUTED, h=47)
        label(m, '›  执行能力详情', 82, 294, 290, 12, INK, h=24)
        shape(m, 'original-right/reload capability', 82, 333, 246, 28, '#f3f3f3', 7)
        label(m, '重新读取执行能力', 94, 339, 230, 12, ACCENT, h=22)
        label(m, '›  Codex 首轮设置与名称', 82, 389, 290, 12, INK, h=25)
        shape(m, 'original-right/isolation checkbox', 82, 447, 14, 14, '#ffffff', 3, '#c9c9c9')
        label(m, '使用独立工作树', 105, 443, 270, 12, INK, h=25)
        label(m, '使用机组的共享工作目录，\n适合知识整理及普通文件。', 82, 480, 290, 11, MUTED, h=46)
        shape(m, 'original-right/captain checkbox', 82, 548, 14, 14, '#ffffff', 3, '#c9c9c9')
        label(m, '设为机长', 105, 544, 270, 12, INK, h=25)
    return master('OriginalRight/64pt Rail and New Member Composer',400,766,draw)

NON_MESSAGE = {6,7,8,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,
               50,51,52,53,54,62,63,64,65}
TERMINAL = {2,11,38,39,40,41,42,43,49,55,56,57,58,59,60,61,66,67,68,69}
affected = []
before_unchanged = {}
for number, board in state_by_number.items():
    if number in NON_MESSAGE:
        before_unchanged[board['do_objectID']] = deepcopy(board)
        continue
    affected.append(number)
    existing = {g['name']: g for g in board['layers']}
    if number == 10:
        # Existing-member rows open their original terminal, not the reused
        # new-member artboard. The inline + controls alone enter that form.
        def member_routes(layer):
            if layer['name'].startswith('member/') and 'flow' in layer:
                layer['flow']['destinationArtboardID'] = boards['单聊']['do_objectID']
            for child in layer.get('layers',[]): member_routes(child)
        for layer in board['layers']: member_routes(layer)
    content = group('04 Content · Original Group Chat and Permanent Member Pane')
    # 1280 = Sidebar238 + chat641 + Divider1 + member400.
    shape(content, 'original-right/work area', 238, 54, 1042, 766, '#ffffff')
    shape(content, 'original-right/fixed divider', 879, 54, 1, 766, LINE)
    # Original group chat header controls, with no redundant member popover.
    symbol_icon(content, 'folder', 516, 70, 17, MUTED, '消息分类')
    label(content, '消息分类  ⌄', 541, 67, 96, 11, MUTED, h=23)
    hotspot(content, 'original-right/category', 510, 61, 131, 33, '消息分类')
    label(content, '人类沟通  ⌄', 653, 67, 116, 11, MUTED, h=23)
    shape(content, 'original-right/mention filter', 783, 65, 74, 25, '#f4f4f4', 8)
    label(content, '仅 @ 你', 796, 70, 64, 11, MUTED, h=20)
    shape(content, 'original-right/chat header divider', 238, 98, 641, .6, LINE)
    label(content, '今天 12:42', 509, 117, 120, 10, '#b2b2b2', h=20)
    shape(content, 'original-right/human bubble', 442, 156, 409, 65, MINT, 18)
    label(content, '先保留原来的群聊与成员工作区。\n其他内容仍从顶部 Tab 查看。', 459, 169, 374, 13, INK, h=45)
    avatar(content, 'captain', 256, 261, 30)
    label(content, '机长', 299, 256, 220, 10, MUTED, h=20)
    label(content, '原群聊一直留在这里。\n右侧可查看工作与成员，或打开原会话。', 299, 283, 535, 13, INK, h=53)
    button(content, '文档 · 原生界面说明', 299, 365, 195, '文档数据')
    button(content, '人待办 · 确认范围', 299, 411, 186, '人待办')
    composer(content, 238, 765, 641, '写群聊消息…')
    if number == 11:
        # Reuse the existing member-detail artboard ID for the actual inline
        # new-member path, rather than presenting the obsolete member popover.
        instance(content,new_member_master(),880,54)
        hotspot(content,'original-right/new member back',880,54,64,55,'群聊')
        hotspot(content,'original-right/new member select original',887,126,50,200,'单聊')
        composer(content,945,765,335,'写下第一件要完成的事…',codex=True)
    elif number in TERMINAL:
        instance(content, terminal_master(), 880, 54)
        hotspot(content, 'original-right/back to members', 880, 54, 64, 55, '群聊')
        hotspot(content, 'original-right/select captain', 887, 126, 50, 60, '单聊')
        hotspot(content, 'original-right/select backend', 887, 194, 50, 60, '单聊')
        hotspot(content, 'original-right/select design', 887, 263, 50, 60, '单聊')
        hotspot(content, 'original-right/new member', 887, 339, 50, 60, '成员详情')
        button(content, '会话控制', 963, 429, 105, 'Codex控制')
        button(content, '检查并恢复', 1080, 429, 119, '恢复确认')
        composer(content, 945, 765, 335, '写给这个原会话…', codex=True)
    else:
        instance(content, roster_master(), 880, 54)
        hotspot(content, 'original-right/Agent ledger', 991, 139, 66, 25, 'AI待办')
        hotspot(content, 'original-right/human ledger', 1061, 139, 60, 25, '人待办')
        hotspot(content, 'original-right/open work', 1200, 139, 66, 27, '人待办')
        ellipse(content,'original-right/new member plus circle',896,448,30,30,MINT,True)
        symbol_icon(content,'plus',903,455,16,ACCENT)
        label(content,'新成员会话',937,454,310,13,INK,h=25)
        hotspot(content,'original-right/new member form',888,444,384,42,'成员详情')
        for i in range(1,4):
            hotspot(content, 'original-right/open original member '+str(i), 896, 479+i*74, 368, 67, '单聊')
    # Preserve native overlay sheets, confirmation controls, Codex four pages,
    # and their existing flows. All backgrounds now use the original two areas.
    board['layers'] = [g for g in board['layers'] if g['name'] != '04 Content'
                       and not (number == 11 and g['name'] == '05 Popovers & Sheets')]
    if number == 11: board['name'] = '11 新成员会话'
    overlay_index = next((i for i,g in enumerate(board['layers'])
                          if g['name'] == '05 Popovers & Sheets'), len(board['layers']))
    board['layers'].insert(overlay_index, content)

assert len(affected) == 43 and len(before_unchanged) == 27
for b in page['layers']:
    if b['do_objectID'] in before_unchanged:
        assert b == before_unchanged[b['do_objectID']]
new_masters = [m for m in masters.values() if m['do_objectID'] not in old_master_ids]
for i,m in enumerate(new_masters):
    m['frame'].update(x=(i%3)*460, y=4000+(i//3)*850)
comp['layers'].extend(new_masters)
page['name'] = 'SwiftUI · Original Right Layout · 70 Editable States'
meta = json.loads(payload['meta.json'])
meta['pagesAndArtboards'][page['do_objectID']]['name'] = page['name']
meta['pagesAndArtboards'][page['do_objectID']]['artboards'] = {
    b['do_objectID']:{'name':b['name']} for b in page['layers']}
payload[pagekey] = json.dumps(page, ensure_ascii=False).encode()
payload[compkey] = json.dumps(comp, ensure_ascii=False).encode()
payload['meta.json'] = json.dumps(meta, ensure_ascii=False).encode()
with zipfile.ZipFile(OUTPUT, 'w', zipfile.ZIP_DEFLATED) as target:
    for name, raw in payload.items(): target.writestr(name, raw)
assert hashlib.sha256(SOURCE.read_bytes()).hexdigest() == SOURCE_SHA
profile = {'file':OUTPUT.name, 'sha256':hashlib.sha256(OUTPUT.read_bytes()).hexdigest(),
           'sourceFile':SOURCE.name,'sourceSHA256':SOURCE_SHA,'AppSourceRef':'431de9dbcec9c41f6d9f11e18d543bf4e7ca076a',
           'artboards':70,'updatedMessageStates':affected,'unchangedStates':sorted(NON_MESSAGE),
           'unchangedArtboardsExact':27,'newSymbols':len(new_masters),
           'reusedMemberDetailArtboard':'11 now represents the actual inline new-member form; ID retained, obsolete popover removed',
           'geometry':{'canvas':[1280,820],'sidebar':238,'groupChat':641,'divider':1,'right':400,'terminalRail':64,'terminalDivider':1,'terminalBody':335},
           'outerDividerDraggable':False,'verticalTodoMemberSplitRetained':True,
           'nativeDoubleClick':False,'freeInputSimulated':False,'AppRuntimeClaim':False,'GUIUsed':False}
(P/'original-right-profile.json').write_text(json.dumps(profile,ensure_ascii=False,indent=2)+'\n')
print(json.dumps(profile,ensure_ascii=False,indent=2))
