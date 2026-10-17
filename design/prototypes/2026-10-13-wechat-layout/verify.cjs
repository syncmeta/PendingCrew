const {chromium}=require('playwright'),{pathToFileURL}=require('url'),fs=require('fs');
(async()=>{
 const b=await chromium.launch({headless:true,executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'}),p=await b.newPage({viewport:{width:1280,height:850}}),errors=[],checks=[];
 p.on('pageerror',e=>errors.push(String(e)));await p.goto(pathToFileURL(__dirname+'/index.html').href);
 async function click(name,want,at={}){const q=p.locator('section:not([hidden])').getByRole('button',{name,exact:true});if(await q.count()>1){const all=await q.all();const valid=[];for(const x of all){const box=await x.boundingBox();if(box&&box.x>=320)valid.push(x)}await valid[0].click()}else await q.click();const got=await p.locator('section:not([hidden])').getAttribute('data-board');if(got!==want)throw Error(name+': '+got);checks.push(name+' → '+want)}
 await click('通讯录','01 通讯录 · 消息');await click('▾ 本机','16 通讯录 · 折叠层级');await click('展开本机','01 通讯录 · 消息');
 await click('Todo','02 通讯录 · Todo');await click('文档','03 通讯录 · 文档');await click('进展','04 通讯录 · 进展');await click('消息','01 通讯录 · 消息');
 const side=p.locator('section:not([hidden])').getByRole('button',{name:'消息',exact:true});await side.first().click();if(await p.locator('section:not([hidden])').getAttribute('data-board')!=='05 消息 · 消息')throw Error('left timeline');checks.push('left 消息 → 时间流');
 await click('单聊 · 后端','09 消息 · 单聊');await click('群聊','05 消息 · 消息');
 await click('Todo','06 消息 · Todo');await click('?  #21 · 是否继续当前工作？','11 消息 · Todo详情');await click('提交答复','12 消息 · 答复后');await click('返回 Todo','06 消息 · Todo');
 await click('文档','07 消息 · 文档');await click('codex-first.md','13 消息 · 文档详情');await click('返回文档','07 消息 · 文档');
 await click('进展','08 消息 · 进展');await click('W12 · 工作工具与检查点','14 消息 · 进展详情');await click('看源 Todo','06 消息 · Todo');
 await click('后端实现','15 消息 · 其他机组');await click('PendingCrew','05 消息 · 消息');
 await p.screenshot({path:__dirname+'/browser-message-layout.png'});
 await click('通讯录','01 通讯录 · 消息');await p.screenshot({path:__dirname+'/browser-contacts-layout.png'});
 if(errors.length)throw Error(errors.join('\n'));fs.writeFileSync(__dirname+'/browser-verification.json',JSON.stringify({status:'PASS',checks,errors},null,2));console.log('PASS',checks.length,'flows; no JS errors');await b.close();
})().catch(e=>{console.error(e);process.exit(1)});
