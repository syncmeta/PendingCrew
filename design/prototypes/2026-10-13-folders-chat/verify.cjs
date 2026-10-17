const {chromium}=require('playwright'),{pathToFileURL}=require('url'),fs=require('fs');
(async()=>{
 const browser=await chromium.launch({headless:true,executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'});
 const p=await browser.newPage({viewport:{width:1480,height:940}}),errors=[],checks=[];
 p.on('pageerror',e=>errors.push(String(e)));await p.goto(pathToFileURL(__dirname+'/index.html').href);
 async function click(name,want){await p.locator('section:not([hidden])').getByRole('button',{name,exact:true}).first().click();const got=await p.locator('section:not([hidden])').getAttribute('data-board');if(got!==want)throw Error(name+': '+got);checks.push(name+' → '+want)}
 async function jump(n){await p.locator('#jump').selectOption({label:n})}
 await click('后端 · PendingCrew    12:08','单聊');await click('群  PendingCrew    12:10','群聊');
 await p.screenshot({path:__dirname+'/browser-chat.png'});
 await click('通讯录','通讯录');await click('▾ 本机','通讯录折叠');await click('▸ 本机','通讯录');await click('     前端设计 ↗','群聊');
 await click('全部消息 ▾','消息分组');await click('新建文件夹','新建分组');await click('保存','分组管理');await click('重命名','重命名分组');await click('保存','分组管理');await click('移动内容','移动内容');await click('▾ 实现讨论','移动完成');await click('返回消息','群聊');
 await click('? #21  等你答复：是否继续当前工作？','Todo详情');await click('提交答复','答复后');await click('返回 Todo','Todo');
 await click('文件夹 ▾','Todo分组');await click('▾ 首版验收','Todo');
 await click('文档','文档');await click('文件夹 ▾','文档分组');await click('▾ 验证记录','文档');await click('reconnect-results.txt','文档详情');await click('关联进展 W12','步骤展开');
 await click('▾ 01 工具验收   已完成','工具折叠');await click('▸ 01 工具验收   已完成','步骤展开');await click('▸ 02 检查点跟进   进行中','检查点展开');await p.screenshot({path:__dirname+'/browser-progress.png'});await click('▾ 02 检查点跟进   进行中','步骤展开');
 await click('产物 · reconnect-results.txt','产物');await click('关联进展 W12','步骤展开');await click('源 Todo #20','Todo20详情');
 await click('进展','进展');await click('▾ W12 · 工作工具与检查点','步骤折叠');await click('▸ W12 · 工作工具与检查点','步骤展开');await click('文件夹 ▾','进展分组');await click('▾ 首版实现','进展');
 await click('搜索当前内容','搜索');await click('W12 · 工作工具与检查点','步骤展开');
 for(const n of await p.locator('#jump option').allTextContents()){await jump(n);const outside=await p.locator('section:not([hidden]) .shape,section:not([hidden]) .txt').evaluateAll(xs=>xs.filter(x=>parseFloat(x.style.left)+parseFloat(x.style.width)>1440||parseFloat(x.style.top)+parseFloat(x.style.height)>860).map(x=>x.textContent||x.getAttribute('aria-label')));if(outside.length)throw Error(n+' out of bounds '+outside)}
 if(errors.length)throw Error(errors.join('\n'));
 fs.writeFileSync(__dirname+'/browser-verification.json',JSON.stringify({status:'PASS',checks,errors,boards:28},null,2));console.log('PASS',checks.length,'flows, 28 board bounds, no JS errors');await browser.close();
})().catch(e=>{console.error(e);process.exit(1)});
