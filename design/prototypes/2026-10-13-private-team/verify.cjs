const {chromium}=require('playwright');
const fs=require('fs');
const {pathToFileURL}=require('url');
(async()=>{
 const browser=await chromium.launch({headless:true,executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'});
 const page=await browser.newPage({viewport:{width:1280,height:850}}), errors=[], checks=[];
 page.on('pageerror',e=>errors.push(String(e)));
 await page.goto(pathToFileURL(__dirname+'/index.html').href);
 async function click(name,expected){await page.locator('section:not([hidden])').getByRole('button',{name,exact:true}).click();if(await page.locator('section:not([hidden])').getAttribute('data-board')!==expected)throw Error(name);checks.push(name+' → '+expected)}
 await click('PendingCrew 开发','02 机组与成员');
 await click('后端 · 工作成员','03 会话');
 await click('后端：工作操作接口已接上，接下来验证 agent 工具。','05 计划详情');
 await click('编辑备注','06 备注编辑');await click('保存备注','07 备注已保存');
 await page.locator('#jump').selectOption({label:'08 待我处理'});
 await click('H18 · 首版恢复后是否继续 W12？','09 答复问题');await click('提交答复','10 答复已提交');
 await page.locator('#jump').selectOption({label:'09 答复问题'});await click('稍后','11 稍后处理');
 await page.locator('#jump').selectOption({label:'09 答复问题'});await click('撤回问题','12 撤回确认');await click('确认撤回','13 已撤回');
 await page.locator('#jump').selectOption({label:'14 恢复提示'});await click('恢复原会话','15 已恢复');
 await page.locator('#jump').selectOption({label:'14 恢复提示'});await click('稍后再说','16 暂不恢复');
 await page.locator('#jump').selectOption({label:'04 工作'});await click('查看冻结工作','18 冻结工作');
 if(await page.locator('section:not([hidden]) .hotspot').count()!==7)throw Error('frozen contains unexpected editing actions');
 await page.locator('#jump').selectOption({label:'01 总览'});
 await page.screenshot({path:__dirname+'/browser-overview.png'});
 if(errors.length)throw Error(errors.join('\n'));
 fs.writeFileSync(__dirname+'/browser-verification.json',JSON.stringify({status:'PASS',checks,errors},null,2));
 console.log('PASS',checks.length,'flows; no JS errors');await browser.close();
})().catch(e=>{console.error(e);process.exit(1)});
