#!/usr/bin/env node
// Development-only visible-DOM journey. No models or private browser profile.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const crypto = require('node:crypto');
const {spawn, spawnSync} = require('node:child_process');
const assert = require('node:assert/strict');
const [modulePath, chromiumPath, sourceBinary, outputParent] = process.argv.slice(2);
if (!outputParent) throw Error('usage: qualify_ui.cjs PLAYWRIGHT_MODULE CHROMIUM_EXECUTABLE BINARY OUTPUT_PARENT');
const {chromium} = require(modulePath);
const sha = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const run = fs.mkdtempSync(path.join(outputParent,'terraforma-ui-'));
fs.mkdirSync(path.join(run,'home'),{mode:0o700});
const binary=path.join(run,'terraforma-local-core');fs.copyFileSync(sourceBinary,binary);fs.chmodSync(binary,0o700);
const env={HOME:path.join(run,'home'),PATH:'/usr/bin:/bin',LANG:'C.UTF-8'};
const processes=[], checks=[], requests=[], errors=[];
let debugPage;
const check=(name,value)=>{assert(value,name);checks.push(name);};
function cli(...args) { const result=spawnSync(binary,args,{cwd:run,env,encoding:'utf8',timeout:10000});assert.equal(result.status,0,result.stderr);return JSON.parse(result.stdout); }
function start(estate) {
  return new Promise((resolve,reject)=>{
    const process=spawn(binary,['serve',estate,'0'],{cwd:run,env,stdio:['ignore','pipe','pipe']});processes.push(process);
    let text=''; const timer=setTimeout(()=>reject(Error('server startup timeout')),10000);
    process.once('error',e=>{clearTimeout(timer);reject(e);});
    process.stdout.on('data',chunk=>{text+=chunk.toString();if(text.includes('\n')){clearTimeout(timer);const ready=JSON.parse(text.split('\n')[0]);if(ready.status==='listening')resolve({process,url:ready.url});}});
    process.stderr.on('data',chunk=>fs.appendFileSync(path.join(run,'server-errors.log'),chunk));
  });
}
async function settle(page) { await page.locator('main[aria-busy="false"]').waitFor(); }
async function click(page,role,name) { await page.getByRole(role,{name,exact:true}).click();await settle(page); }
async function find(page,query) { await page.getByLabel('Find in your saved documents').fill(query);await click(page,'button','Search'); }
async function stop(page,server) { await click(page,'button','Stop workspace');await page.locator('#connection').filter({hasText:'Stopped'}).waitFor();await new Promise((resolve,reject)=>{if(server.process.exitCode!==null)return resolve();const t=setTimeout(()=>reject(Error('stop timeout')),6000);server.process.once('exit',()=>{clearTimeout(t);resolve();});}); }

(async()=>{
  const browser=await chromium.launch({headless:true,executablePath:chromiumPath});
  try {
    const context=await browser.newContext({viewport:{width:1280,height:900},acceptDownloads:true});
    context.on('request',r=>requests.push(r.url()));
    const page=await context.newPage(); debugPage=page;
    page.on('pageerror',e=>errors.push(e.message));
    page.on('dialog',async d=>{if(d.type()==='confirm' && d.message().startsWith('Restore the documents'))await d.accept();else{errors.push('unexpected dialog: '+d.message());await d.dismiss();}});
    const estate=path.join(run,'estate');cli('init',estate,'ui-rehearsal');
    let server=await start(estate);await page.goto(server.url);await settle(page);
    check('empty_workspace_shows_sample_entry',await page.getByRole('button',{name:'Try a sample',exact:true}).isVisible());
    await page.screenshot({path:path.join(run,'01-empty.png'),fullPage:true});
    await click(page,'button','Try a sample');
    check('sample_is_reviewed_before_admission',(await page.locator('#count').textContent())==='0' && await page.getByLabel('Document text',{exact:true}).isVisible());
    await click(page,'button','Save document');
    check('sample_saved_and_source_opened',(await page.locator('#source-text').textContent()).includes('2400 EUR'));
    await find(page,'project 7');
    check('search_renders_full_match',(await page.locator('#hits').textContent()).includes('ALL WORDS'));
    await page.locator('#hits').getByRole('button',{name:'Open saved source →',exact:true}).first().click();await settle(page);
    check('search_opens_exact_retained_source',(await page.locator('#source-title').textContent())==='Maple studio · project 7');
    await find(page,'project 7 unicorn');check('partial_result_labels_missing_words',(await page.locator('#hits').textContent()).includes('Not found here: unicorn'));
    await find(page,'quantum zebra');check('no_match_does_not_invent_answer',(await page.locator('#result-heading').textContent())==='No matching source');

    const stale=await context.newPage();stale.on('pageerror',e=>errors.push(e.message));
    await stale.goto(server.url);await settle(stale);
    await stale.locator('#documents button').filter({hasText:'Maple studio'}).click();await settle(stale);
    await click(stale,'button','Edit saved copy');
    await stale.getByLabel('Document text',{exact:true}).fill('Project 7 stale draft with 2800 EUR.');
    await page.locator('#documents button').filter({hasText:'Maple studio'}).click();await settle(page);
    await click(page,'button','Edit saved copy');
    const baseText=await page.getByLabel('Document text',{exact:true}).inputValue();
    await page.getByLabel('Document text',{exact:true}).fill(baseText.replace('2400','3000'));await click(page,'button','Save document');
    await click(stale,'button','Save document');
    check('stale_tab_keeps_draft_and_requests_review',await stale.locator('#conflict').isVisible() && (await stale.getByLabel('Document text',{exact:true}).inputValue()).includes('2800') && (await stale.locator('#latest-text').textContent()).includes('3000'));
    await stale.close();

    const file=path.join(run,'chosen-note.txt');const bytes=Buffer.from('\uFEFFProject 81\r\nThe copper delivery arrives on 23 September.\r\n');fs.writeFileSync(file,bytes);
    async function choose() {const pending=page.waitForEvent('filechooser');await page.getByRole('button',{name:'Add a text file',exact:true}).click();await (await pending).setFiles(file);await settle(page);}
    await choose();check('chosen_file_is_previewed',(await page.getByLabel('Title',{exact:true}).inputValue())==='chosen-note.txt');
    await click(page,'button','Save document');
    const proof=JSON.parse(await page.locator('#source-proof').textContent());
    check('file_bom_crlf_and_sha256_preserved',proof.source_sha256===sha(file) && (await page.locator('#source-text').textContent())===bytes.toString('utf8'));
    await choose();await click(page,'button','Save document');await find(page,'81');
    check('duplicate_files_presented_once',(await page.locator('#hits article').count())===1 && (await page.locator('#hits').textContent()).includes('2 saved copies'));
    await page.locator('#hits button').first().click();await settle(page);
    await click(page,'button','Edit saved copy');
    const pending=page.waitForEvent('filechooser');await page.getByRole('button',{name:'Choose replacement text',exact:true}).click();
    const invalid=path.join(run,'invalid.txt');fs.writeFileSync(invalid,Buffer.from([0xff,0xfe]));await (await pending).setFiles(invalid);await settle(page);
    check('invalid_utf8_does_not_replace_draft',(await page.locator('#notice').textContent()).includes('not valid UTF-8') && (await page.getByLabel('Document text',{exact:true}).inputValue()).includes('23 September'));
    await click(page,'button','Cancel');
    await page.getByText('Back up and recover',{exact:true}).click();
    await click(page,'button','Create checkpoint');
    await click(page,'button','Edit saved copy');
    await page.getByLabel('Document text',{exact:true}).fill('Project 81\nThe copper delivery arrives on 27 September.\n');
    await click(page,'button','Save document');await find(page,'27');
    check('edited_text_immediately_visible_in_search',(await page.locator('#result-heading').textContent())==='Matching sources');
    const backupPath=path.join(run,'downloaded-backup.json');
    const downloadPromise=page.waitForEvent('download');await page.getByRole('button',{name:'Download backup',exact:true}).click();
    const download=await downloadPromise;await download.saveAs(backupPath);await settle(page);
    check('backup_download_delivers_file',fs.existsSync(backupPath) && JSON.parse(fs.readFileSync(backupPath)).schema==='ubos.terraforma-local-backup.v1');
    await click(page,'button','Restore checkpoint');await find(page,'27');
    check('checkpoint_restore_changes_visible_search',(await page.locator('#result-heading').textContent())==='No matching source');
    await stop(page,server);check('stop_button_closes_process',server.process.exitCode===0);
    server=await start(estate);await page.goto(server.url);await settle(page);await find(page,'27');
    check('restart_replays_restored_checkpoint',(await page.locator('#result-heading').textContent())==='No matching source');
    await stop(page,server);
    const recovered=path.join(run,'recovered');cli('restore-backup',backupPath,recovered);
    server=await start(recovered);await page.goto(server.url);await settle(page);await find(page,'27');
    await page.locator('#hits button').first().click();await settle(page);
    check('downloaded_backup_restores_browser_source',(await page.locator('#source-text').textContent()).includes('27 September'));
    await page.screenshot({path:path.join(run,'02-restored.png'),fullPage:true});
    await click(page,'button','Write a note');
    await page.getByLabel('Title',{exact:true}).fill('<img src=x onerror=alert(1)>');
    await page.getByLabel('Document text',{exact:true}).fill('<script>alert(1)</script>\nhttps://example.invalid/private-source');
    await click(page,'button','Save document');
    check('source_markup_displayed_as_inert_text',(await page.locator('#source-text').textContent()).includes('<script>') && await page.locator('#source img,#source script,#documents img').count()===0);
    await find(page,'project 7');await page.locator('#hits button').first().click();await settle(page);
    await page.screenshot({path:path.join(run,'03-desktop.png'),fullPage:true});
    await page.setViewportSize({width:390,height:844});
    await page.locator('#hits button').first().click();await settle(page);
    check('narrow_source_open_scrolls_to_document',await page.locator('#source').evaluate(e=>{
      const b=e.getBoundingClientRect(), text=e.querySelector('#source-text').getBoundingClientRect();
      // Near the end of the page the browser cannot align the card to y=0.
      return b.top>=0 && b.top<window.innerHeight/2 && text.top<window.innerHeight;
    }));
    await page.screenshot({path:path.join(run,'04-mobile.png'),fullPage:true});
    check('narrow_layout_has_no_horizontal_overflow',await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth));
    check('browser_has_no_javascript_errors',errors.length===0);
    check('page_makes_no_external_requests',requests.every(u=>u.startsWith('http://127.0.0.1:') || u.startsWith('blob:http://127.0.0.1:')));
    await stop(page,server);
    const screenshots=['01-empty.png','02-restored.png','03-desktop.png','04-mobile.png'].map(name=>({path:path.join(run,name),sha256:sha(path.join(run,name))}));
    const receipt={schema:'ubos.terraforma-browser-ui-qualification.v1',status:'PASS_ISOLATED_CHROMIUM_M4',observed_at:new Date().toISOString(),run_directory:run,browser:browser.version(),binary:{path:binary,sha256:sha(binary),bytes:fs.statSync(binary).size},checks,check_count:checks.length,backup:{path:backupPath,sha256:sha(backupPath)},screenshots,source_fixture:{path:file,sha256:sha(file)},harness_sha256:sha(__filename),page_external_requests:[],page_errors:errors,boundary:'Synthetic UI journey in isolated local Chromium. Managed Codex/Chrome tool launchers refused the rehearsal URL; no in-app launch proof. No installer, clean-OS, Linux, Windows or unrelated-user qualification.'};
    const receiptPath=path.join(run,'qualification.json');fs.writeFileSync(receiptPath,JSON.stringify(receipt,null,2)+'\n');
    console.log(JSON.stringify({status:receipt.status,checks:checks.length,receipt:receiptPath,sha256:sha(receiptPath)},null,2));
  } catch (e) {
    if(debugPage) {
      await debugPage.screenshot({path:path.join(run,'failure.png'),fullPage:true}).catch(()=>{});
      const diagnostic={run,checks,errors,notice:await debugPage.locator('#notice').textContent().catch(()=>''),source:await debugPage.locator('#source-text').textContent().catch(()=>''),draft:await debugPage.locator('#text').inputValue().catch(()=>''),title:await debugPage.locator('#title').inputValue().catch(()=>''),message:e.message};
      fs.writeFileSync(path.join(run,'failure.json'),JSON.stringify(diagnostic,null,2)+'\n'); console.error(JSON.stringify(diagnostic));
    }
    throw e;
  } finally {for(const p of processes)if(p.exitCode===null)p.kill('SIGTERM');await browser.close();}
})().catch(e=>{console.error(e.stack);process.exitCode=1;});
