const {chromium}=require('playwright');
const {pathToFileURL}=require('node:url');
const fs=require('node:fs');
const assert=require('node:assert/strict');
const topics=require('./src/topicCrews.json');
(async()=>{
 const browser=await chromium.launch({headless:true,executablePath:process.env.CHROME_PATH||(process.platform==='darwin'?'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome':undefined)});
 try {
  const page=await browser.newPage({viewport:{width:1500,height:1050}}),errors=[];
  page.on('pageerror',e=>errors.push(String(e)));
  await page.goto(pathToFileURL(__dirname+'/portable/index.html').href);
  await page.evaluate(()=>localStorage.clear());await page.reload();
  const checks=[];
  for(const [id,groups] of Object.entries(topics)) {
   await page.locator('.sidebar .crew-row[data-id="'+id+'"]').first().click();
   assert.deepEqual(await page.locator('.conv-title').allTextContents(),groups.map(g=>g.title));
   for(const group of groups.slice(1)) {
    await page.locator('.conv-row[data-id="'+id+'-'+group.id+'"]').click();
    assert.deepEqual(await page.locator('.member-row .name').allTextContents(),[group.host,'人']);
    const body=await page.locator('.messages').innerText();
    assert(!body.includes('这个 Demo 使用 React'));assert(!body.includes('这份演示直接使用'));
    assert.equal(await page.locator('.messages .message').count(),group.documents.length);
    assert((await page.locator('.messages .ref-link').count())>=1);
    await page.locator('.messages .ref-link').first().click();
    assert(await page.locator('.document-content').isVisible());
    assert((await page.locator('.document-sources a').count())>=1);
    await page.locator('[data-tab="message"]').click();
    await page.locator('.member-row').first().click();
    assert.equal(await page.locator('.conv-row.selected').getAttribute('data-id'),id+'-'+group.id);
   }
   await page.screenshot({path:__dirname+'/evidence/topic-'+id+'.png'});
   checks.push(id+': topical groups, distinct members, full source content and working document references');
  }
  assert.equal(errors.length,0,errors.join('\n'));
  fs.writeFileSync(__dirname+'/evidence/topics-verification.json',JSON.stringify({status:'PASS',checks,errors},null,2));
  console.log('PASS 3 topic crew checks');
 } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exit(1)});
