'use strict';
const $ = id => document.getElementById(id);
const token = document.querySelector('meta[name="workspace-token"]').content;
document.querySelector('meta[name="workspace-token"]').remove();
let current, opened, draftId, draftHead, reviewedHead, draftRawText = '', draftDisplayedText = '', replacing = false, busy = false;
const size = text => new TextEncoder().encode(text).length;
// Textareas normalize CRLF. Preserve the selected UTF-8 bytes until text is edited.
const editorText = () => $('text').value === draftDisplayedText ? draftRawText : $('text').value;
function setDraftText(text) { draftRawText=text; $('text').value=text; draftDisplayedText=$('text').value; }
function notice(message, error = false) { $('notice').textContent = message; $('notice').className = error ? 'error' : ''; $('notice').hidden = false; }
async function api(action, fields = {}) {
  let response;
  try { response = await fetch('/api', {method:'POST', headers:{'Content-Type':'application/json','X-Workspace-Token':token}, body:JSON.stringify({action,...fields})}); }
  catch { $('connection').textContent = 'Disconnected'; $('connection').classList.add('off'); throw Error('Workspace is not running. Start it in the terminal, then reload this page. Your draft is still here.'); }
  const value = await response.json();
  if (!response.ok) { const error = Error(value.error || 'The action could not be completed.'); error.status = response.status; throw error; }
  return value;
}
function run(fn) { return async event => { if (event) event.preventDefault(); if (busy) return; busy = true; document.querySelector('main').setAttribute('aria-busy','true'); try { await fn(); } catch (e) { notice(e.message,true); } finally { busy = false; document.querySelector('main').setAttribute('aria-busy','false'); } }; }
function button(label, fn, className = 'text-button') { const b = document.createElement('button'); b.type = 'button'; b.textContent = label; b.className = className; b.addEventListener('click',run(fn)); return b; }
function element(tag, text, className) { const e = document.createElement(tag); e.textContent = text; if (className) e.className = className; return e; }
function panel(name) { for (const id of ['welcome','source','editor']) $(id).hidden = id !== name; }
async function refresh() {
  current = await api('status');
  $('connection').textContent = `Saved locally · ${current.records.length} documents`;
  $('connection').classList.remove('off'); $('count').textContent = current.records.length;
  $('empty').hidden = current.records.length > 0;
  $('documents').replaceChildren();
  for (const r of current.records) {
    const b = button('',() => openRecord(r.id,r.source_sha256),'saved');
    b.append(element('strong',r.title),element('small',`${r.bytes.toLocaleString()} bytes · Saved copy`));
    $('documents').append(b);
  }
  $('ring').replaceChildren();
  for (let i=0;i<current.status.ring;i++) { const option = element('option',`Checkpoint ${i+1}`); option.value=i; $('ring').append(option); }
  $('rollback').disabled = !current.status.ring;
  $('footer-status').textContent = `Local rehearsal ${current.version} · ${current.records.length} / ${current.limits.documents} documents · No external connections`;
}
async function search() {
  const q = $('query').value.trim(); if (!q) return;
  const envelope = await api('query',{query:q}), result=envelope.result;
  $('results').hidden=false; $('hits').replaceChildren();
  $('result-heading').textContent = result.status==='no_match' ? 'No matching source' : result.status==='partial_match' ? 'Some words matched' : 'Matching sources';
  $('result-summary').textContent = result.status==='no_match' ? 'None of these words appear in the saved text or titles. Try another word, or add the source you need.' : `${result.hits.length} unique text result${result.hits.length===1?'':'s'}${result.truncated?' · More matches exist; narrow your search':''}. Open a source to check the details.`;
  for (const h of result.hits) {
    const card=element('article','', 'hit');
    const title=element('h3',h.title), badge=element('span',h.status==='full_match'?'ALL WORDS':'PARTIAL','badge'+(h.status==='full_match'?'':' partial'));
    card.append(badge,title,element('p',h.excerpt || 'The match is in the title.'),element('p',`Matched: ${h.matched_terms.join(', ')}${h.missing_terms.length?' · Not found here: '+h.missing_terms.join(', '):''}`,'coverage'));
    if (h.source_ids_with_identical_text.length>1) card.append(element('p',`${h.source_ids_with_identical_text.length} saved copies contain identical text; shown once.`,'hint'));
    card.append(button('Open saved source →',()=>openRecord(h.id,h.source_sha256)));
    $('hits').append(card);
  }
  $('query-receipt').textContent=JSON.stringify(envelope,null,2);
}
async function openRecord(id, hash) {
  const value=await api('read',{id,expected_source_sha256:hash}); opened=value.record;
  $('source-title').textContent=opened.title; $('source-text').textContent=opened.text;
  $('source-meta').textContent=`${size(opened.text).toLocaleString()} bytes · Exact saved text`;
  $('source-proof').textContent=JSON.stringify({id:opened.id,source_sha256:opened.source_sha256,world_sha256:value.world_sha256},null,2);
  panel('source');
  if (window.matchMedia('(max-width:780px)').matches) $('source').scrollIntoView({block:'start'});
}
function draft(record = null) {
  draftId=record?.id || crypto.randomUUID(); draftHead=current.status.journal_head_sha256;
  $('title').value=record?.title || ''; setDraftText(record?.text || '');
  $('editor-kind').textContent=record?'UPDATE SAVED COPY':'NEW DOCUMENT';
  $('conflict').hidden=true; panel('editor'); countBytes(); $('title').focus();
}
function countBytes() { $('byte-count').textContent=`${size(editorText()).toLocaleString()} / 16,384 bytes`; }
async function save() {
  if (size(editorText())>16384 || size($('title').value)>256) throw Error('Use a shorter title (256 bytes) and text no larger than 16 KiB. Your draft is kept.');
  let receipt;
  try { receipt=await api('put',{id:draftId,title:$('title').value,text:editorText(),expected_head:draftHead}); }
  catch (e) {
    if (e.status===409) {
      await refresh(); reviewedHead=current.status.journal_head_sha256;
      const saved=current.records.find(r=>r.id===draftId);
      if (saved) { const latest=await api('read',{id:draftId,expected_source_sha256:saved.source_sha256}); $('latest-text').textContent=latest.record.text; reviewedHead=latest.journal_head_sha256; }
      else $('latest-text').textContent='This document has not been saved yet. Other workspace changes have been refreshed.';
      $('conflict').hidden=false;
    }
    throw e;
  }
  await refresh(); await openRecord(draftId); await search();
  notice(receipt.status==='unchanged'?'This exact text was already saved. No duplicate revision was created.':'Saved locally. Search now includes this version.');
}
function chooseFile(replace=false) { replacing=replace; $('file').value=''; $('file').click(); }
$('file').addEventListener('change',run(async()=>{
  const f=$('file').files[0]; if(!f) return;
  if (!/\.(txt|md|csv|json|rs|py|js|ts|html|css|toml|yaml|yml|log)$/i.test(f.name)) throw Error('Choose a supported text file. PDF, Word and scanned files are not supported yet.');
  if(f.size>16384 || f.size===0) throw Error('Choose a nonempty UTF-8 text file no larger than 16 KiB.');
  let text; try { text=new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}).decode(await f.arrayBuffer()); } catch { throw Error('This file is not valid UTF-8 text. PDF, Word and scanned files are not supported yet.'); }
  if (!replacing) draft({id:crypto.randomUUID(),title:f.name,text}); else { setDraftText(text); countBytes(); }
  $('editor-kind').textContent=replacing?'REVIEW REPLACEMENT TEXT':'REVIEW SELECTED FILE';
  notice(`Loaded ${f.name} for review. Nothing is saved until you choose Save document.`);
}));
$('sample').addEventListener('click',run(async()=>{ draft({id:crypto.randomUUID(),title:'Maple studio · project 7',text:'Project 7 — Maple studio\nThe review meeting is on 18 September.\nThe agreed budget is 2400 EUR.\nBring the revised floor plan to the review.\nThis is a fictional example for trying the workspace.'}); $('query').value='project 7'; notice('This is a sample draft. Save it, then try searching for project 7.'); }));
for (const id of ['choose-file','add-file']) $(id).addEventListener('click',()=>chooseFile());
$('replace-file').addEventListener('click',()=>chooseFile(true));
$('new-note').addEventListener('click',run(async()=>draft()));
$('edit').addEventListener('click',run(async()=>{
  await refresh();
  const latest=await api('read',{id:opened.id,expected_source_sha256:opened.source_sha256});
  draft(latest.record); draftHead=latest.journal_head_sha256;
}));
$('cancel-edit').addEventListener('click',()=>panel(opened?'source':'welcome'));
$('text').addEventListener('input',countBytes);
$('document-form').addEventListener('submit',run(save));
$('accept-current').addEventListener('click',()=>{draftHead=reviewedHead;$('conflict').hidden=true;notice('Your draft is kept. Choose Save document to make it the next revision.');});
$('search-form').addEventListener('submit',run(search));
$('clear-search').addEventListener('click',()=>{$('query').value='';$('results').hidden=true;});
$('refresh').addEventListener('click',run(async()=>{await refresh();await search();notice('Workspace refreshed. Only explicitly saved copies are searched.');}));
$('checkpoint').addEventListener('click',run(async()=>{await api('checkpoint',{expected_head:current.status.journal_head_sha256});await refresh();notice(`Checkpoint ${current.status.ring} created. Your next edits can be rolled back to it.`);}));
$('rollback').addEventListener('click',run(async()=>{
  if (!confirm('Restore the documents at this checkpoint? Later edits will leave the current view; their history remains in backups.')) return;
  await api('rollback',{ring:Number($('ring').value),expected_head:current.status.journal_head_sha256});
  await refresh();await search();opened=null;panel('welcome');notice('Checkpoint restored. Search reflects the restored text.');
}));
$('backup').addEventListener('click',run(async()=>{
  const bundle=await api('backup'), blob=new Blob([JSON.stringify(bundle)],{type:'application/json'}), url=URL.createObjectURL(blob);
  const a=document.createElement('a');a.href=url;a.download=`${current.status.estate_id}.terraforma-backup.json`;document.body.append(a);a.click();a.remove();setTimeout(()=>URL.revokeObjectURL(url),5000);
  notice('Backup prepared and handed to your browser for download. It includes saved documents and their history.');
}));
$('stop').addEventListener('click',run(async()=>{
  await api('stop');$('connection').textContent='Stopped · documents kept';$('connection').classList.add('off');
  for(const e of document.querySelectorAll('button,input,textarea,select')) e.disabled=true;
  notice('Workspace stopped. Your saved documents remain on disk. Start it again from the terminal to return.');
}));
run(refresh)();
