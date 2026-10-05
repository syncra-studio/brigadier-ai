import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE ?? "playwright");
const out=process.env.INLINE_PASTE_SHOTS ?? "/tmp/brigadier-inline-paste-shots";
await mkdir(out,{recursive:true});
const browser=await chromium.launch();
try {
const page=await browser.newPage({viewport:{width:1100,height:780}});
page.on('pageerror',e=>console.error('PAGE ERROR',e.message));
await page.goto(`${process.env.INLINE_PASTE_URL ?? "http://localhost:1483"}/fixtures/inline-paste.html?view=done&summary=0&sidebar=0`);
page.setDefaultTimeout(15000);
const input=page.getByRole('textbox',{name:'Message input'});
await page.locator(".aui-lexical-input").waitFor();
await input.click();
await input.pressSequentially('Compare ');
await page.waitForTimeout(100);
async function selectAllInput() {
 await input.focus();
 await input.evaluate(element=>{const selection=window.getSelection();const range=document.createRange();range.selectNodeContents(element);selection.removeAllRanges();selection.addRange(range)});
 await page.waitForTimeout(50);
}
async function pasteImage(color='red',age=0) {
 await input.evaluate((element,{color: pasteColor,age: fileAge})=>{
  const canvas=document.createElement('canvas');canvas.width=80;canvas.height=60;
  const ctx=canvas.getContext('2d');ctx.fillStyle=pasteColor;ctx.fillRect(0,0,80,60);
  const data=atob(canvas.toDataURL('image/png').split(',')[1]);
  const file=new File([Uint8Array.from(data,c=>c.charCodeAt(0))],'image.png',{type:'image/png',lastModified:Date.now()-fileAge});
  const clipboardData=new DataTransfer();clipboardData.items.add(file);
  element.dispatchEvent(new ClipboardEvent('paste',{clipboardData,bubbles:true,cancelable:true}));
 },{color,age});
}
await pasteImage();
await page.getByRole('button',{name:'Preview Image #1',exact:true}).waitFor();
await page.waitForFunction(()=>document.querySelector('[data-slot="inline-image"] img')?.naturalWidth>0);
assert.equal(await page.locator('[data-slot="composer-attachment"]').count(),0);
await page.screenshot({path:out+'/composer-one.png'});
await input.press('End');
await input.press('ArrowRight');
await pasteImage();
await page.waitForFunction(()=>document.querySelectorAll('[aria-label="Preview Image #1"]').length===2);
await page.screenshot({path:out+'/composer-two-copies.png'});
console.log('PASS same bytes yield two chips named Image #1; no row attachments');
// Enter and Space activate chip controls without sending.
await page.getByRole('button',{name:'Preview Image #1',exact:true}).first().focus();
await page.keyboard.press('Enter');
await page.locator('[data-slot="image-preview"]').waitFor();
assert.equal(await page.evaluate(()=>window.inlinePaste.calls.filter(c=>c.method==='sendMessage').length),0);
await page.keyboard.press('Escape');
await page.getByRole('button',{name:'Preview Image #1',exact:true}).first().focus();
await page.keyboard.press('Space');
await page.locator('[data-slot="image-preview"]').waitFor();
await page.keyboard.press('Escape');
console.log('PASS Enter and Space open preview instead of sending');
await page.getByRole('button',{name:'Remove Image #1',exact:true}).first().focus();
await page.keyboard.press('Enter');
await page.waitForFunction(()=>document.querySelectorAll('[aria-label="Preview Image #1"]').length===1);
await input.press('Meta+z');
await page.waitForFunction(()=>document.querySelectorAll('[aria-label="Preview Image #1"]').length===2);
console.log('PASS remove one copy and undo');
await input.focus();
await input.press('End');
await input.press('Enter');
await page.waitForFunction(()=>window.inlinePaste.calls.some(c=>c.method==='sendMessage'));
const sent=await page.evaluate(()=>window.inlinePaste.calls.find(c=>c.method==='sendMessage'));
assert.equal(sent.attachments.length,1);
assert.equal(sent.attachments[0].inline,1);
assert.equal((sent.text.match(/\[Image #1\]/g)||[]).length,2);
await page.waitForFunction(()=>document.querySelector('[data-slot="aui_user-message-root"]'));
await page.screenshot({path:out+'/sent-bubble.png'});
console.log('PASS sent two positions, one attachment, sent bubble has AB chips');
console.log('SENT',JSON.stringify(sent));

assert.equal(await page.locator('[data-slot="aui_user-message-root"] [data-slot="inline-image"]').count(),2);
assert.equal(await page.locator('[data-slot="aui_user-message-root"] [aria-label^="Remove Image"]').count(),0);
// New message: copying/cutting uses synthetic clipboard data, without touching the OS clipboard.
await input.focus();
await pasteImage('blue');
await page.getByRole('button',{name:'Remove Image #1',exact:true}).waitFor();
await page.waitForTimeout(100);
await selectAllInput();
await input.evaluate(element=>{
 const clipboardData=new DataTransfer();window.testClipboard=clipboardData;
 element.dispatchEvent(new ClipboardEvent('cut',{clipboardData,bubbles:true,cancelable:true}));
});
await page.waitForFunction(()=>document.querySelector('.aui-lexical-input').textContent==='');
await input.evaluate(element=>element.dispatchEvent(new ClipboardEvent('paste',{clipboardData:window.testClipboard,bubbles:true,cancelable:true})));
await page.waitForTimeout(500);
await page.getByRole('button',{name:'Remove Image #1',exact:true}).waitFor();
await page.waitForTimeout(100);
await selectAllInput();
await input.evaluate(element=>{
 const clipboardData=new DataTransfer();window.testClipboard=clipboardData;
 element.dispatchEvent(new ClipboardEvent('copy',{clipboardData,bubbles:true,cancelable:true}));
});
await input.evaluate(element=>{ const selection=window.getSelection();const range=document.createRange();range.selectNodeContents(element);range.collapse(false);selection.removeAllRanges();selection.addRange(range); });
await page.waitForTimeout(50);
await input.evaluate(element=>element.dispatchEvent(new ClipboardEvent('paste',{clipboardData:window.testClipboard,bubbles:true,cancelable:true})));
await page.waitForTimeout(500);
await page.waitForFunction(()=>document.querySelectorAll('.aui-lexical-input [aria-label="Preview Image #1"]').length===2);
console.log('PASS cut/paste restores bytes; copying chip preserves its number and one attachment');
await selectAllInput();await input.press('Backspace');
await page.waitForFunction(()=>document.querySelector('.aui-lexical-input').textContent==='');
// An unrelated clipboard with identical marker text cannot recover the old image.
await input.evaluate(element=>{
 const clipboardData=new DataTransfer();clipboardData.setData('text/plain','[Image #1]');
 element.dispatchEvent(new ClipboardEvent('paste',{clipboardData,bubbles:true,cancelable:true}));
});
assert.equal(await input.textContent(),'Image #1');
assert.equal(await input.locator('[data-slot="inline-image"]').count(),0);
console.log('PASS unrelated marker-only paste becomes words');
await selectAllInput();await input.press('Backspace');
await pasteImage('green',5000);
await page.locator('[data-slot="composer-attachment"]').waitFor();
assert.equal(await input.locator('[data-slot="inline-image"]').count(),0);
console.log('PASS copied image file goes to the attachment row');
await page.locator('[data-slot="composer-attachment"] button[aria-label^="Remove"]').click();
// Guard both sides of duplicate matching against failed uploads.
await page.evaluate(()=>window.inlinePaste.setFail(true));
await pasteImage('purple');
await page.locator('[data-slot="composer-attachment"][data-state="error"]').waitFor();
await page.evaluate(()=>window.inlinePaste.setFail(false));
await pasteImage('purple');
await page.waitForFunction(()=>document.querySelector('.aui-lexical-input').textContent.includes('[Image #') || document.querySelectorAll('.aui-lexical-input [data-slot="inline-image"]').length===2);
await page.waitForTimeout(500);
assert.equal(await page.getByRole('button',{name:'Remove Image #2',exact:true}).count(),1);
await page.getByRole('button',{name:'Remove Image #1',exact:true}).click();
await input.press('Enter');
await page.waitForFunction(()=>window.inlinePaste.calls.filter(c=>c.method==='sendMessage').length===2);
assert.equal((await page.evaluate(()=>window.inlinePaste.calls.filter(c=>c.method==='sendMessage').at(-1))).attachments.length,1);
console.log('PASS stored copy never matches a failed first copy');
// Pasting and immediately sending while the upload is pending must keep the image.
await page.evaluate(()=>window.inlinePaste.setDelay(300));
await pasteImage('orange');
await input.press('Enter');
await page.waitForTimeout(50);
assert.equal(await page.evaluate(()=>window.inlinePaste.calls.filter(c=>c.method==='sendMessage').length),2);
await page.waitForTimeout(400);
await input.press('Enter');
await page.waitForFunction(()=>window.inlinePaste.calls.filter(c=>c.method==='sendMessage').length===3);
assert.equal((await page.evaluate(()=>window.inlinePaste.calls.filter(c=>c.method==='sendMessage').at(-1))).attachments.length,1);
console.log('PASS Enter while uploading cannot lose the image');
await page.evaluate(()=>window.inlinePaste.setDelay(0));
await pasteImage('cyan');
await page.getByRole('button',{name:'Remove Image #1',exact:true}).waitFor();
await page.waitForFunction(()=>JSON.parse(localStorage.getItem('brigadier.drafts')||'{}')['flow-session']?.attachments.length>0);
await page.reload();
await page.locator('.aui-lexical-input').waitFor();
await page.getByRole('button',{name:'Remove Image #1',exact:true}).waitFor();
console.log('PASS saved draft restores the numbered chip');
await page.evaluate(()=>{document.documentElement.dataset.density='compact'});
await page.waitForFunction(()=>document.querySelector('.aui-lexical-input img')?.naturalWidth>0);
await page.screenshot({path:out+'/composer-compact.png'});
// Start with a fresh composer for the opposite failed-copy order and focus guard.
await page.evaluate(()=>localStorage.removeItem('brigadier.drafts'));
await page.reload();await page.locator('.aui-lexical-input').waitFor();
await input.click();await pasteImage('cyan');await page.waitForTimeout(150);
await page.evaluate(()=>window.inlinePaste.setDelay(150));
await pasteImage('cyan');
await page.getByRole('button',{name:'Chat actions',exact:true}).focus();
await page.evaluate(()=>{window.focusBefore=document.activeElement});
await page.waitForTimeout(400);
assert.equal(await page.evaluate(()=>document.activeElement===window.focusBefore), true);
assert.equal(await input.locator('[aria-label="Preview Image #1"]').count(),2);
await input.click();await input.press('End');
await page.evaluate(()=>window.inlinePaste.setFail(true));
await pasteImage('cyan');
await page.waitForTimeout(500);
await page.locator('[data-slot="composer-attachment"][data-state="error"]').waitFor();
assert.equal(await input.locator('[data-slot="inline-image"]').count(),3);
assert.equal(await page.getByRole('button',{name:'Remove Image #2',exact:true}).count(),1);
console.log('PASS later failed copy stays separate and delayed matching keeps focus outside');

await page.evaluate(()=>localStorage.removeItem('brigadier.drafts'));
await page.reload();await page.locator('.aui-lexical-input').waitFor();
await input.fill('x'.repeat(5100)+' ');
await pasteImage('black');await page.waitForTimeout(150);
await selectAllInput();
await input.evaluate(element=>{
 const clipboardData=new DataTransfer();window.testClipboard=clipboardData;
 element.dispatchEvent(new ClipboardEvent('copy',{clipboardData,bubbles:true,cancelable:true}));
});
await input.evaluate(element=>{const selection=window.getSelection();const range=document.createRange();range.selectNodeContents(element);range.collapse(false);selection.removeAllRanges();selection.addRange(range)});
await page.waitForTimeout(50);
await input.evaluate(element=>element.dispatchEvent(new ClipboardEvent('paste',{clipboardData:window.testClipboard,bubbles:true,cancelable:true})));
await page.waitForFunction(()=>document.querySelectorAll('.aui-lexical-input [aria-label="Preview Image #1"]').length===2);
assert.equal(await page.locator('[data-slot="composer-attachment"]').count(),0);
console.log('PASS copied selections over 5000 characters keep their chips and bytes');

// Feed both historical message formats through the current sent-message renderer.
await page.evaluate(async ()=>{
 const stored=window.inlinePaste.calls.find(call=>call.method==='addAttachment');
 const binary=atob(stored.data);
 const hash=await crypto.subtle.digest('SHA-256',Uint8Array.from(binary,char=>char.charCodeAt(0)));
 const id=[...new Uint8Array(hash)].map(byte=>byte.toString(16).padStart(2,'0')).join('');
 const ref={id,name:stored.name,mime:stored.mime,bytes:binary.length,pasted:false};
 const messages=[
  {id:'legacy-main',seq:51,text:`Old main before [image:${id}] after`,attachments:[{...ref,inline:0}]},
  {id:'saved-ab',seq:52,text:'Saved AB before [Image #1] after',attachments:[{...ref,inline:1}]},
 ].map(message=>({...message,conversationId:'flow-session',role:'user',blob:null,mentions:[],createdAtMs:Date.now(),model:null,requestId:message.id,parentId:null}));
 window.flow.useApp.setState(state=>({threads:{...state.threads,'flow-session':{...state.threads['flow-session'],items:messages}}}));
});
await page.waitForFunction(()=>document.querySelectorAll('[data-slot="aui_user-message-root"] [data-slot="inline-image"]').length===2);
assert.equal(await page.locator('[data-slot="aui_user-message-root"]').filter({hasText:'Old main before'}).locator('[data-slot="inline-image"]').count(),1);
assert.equal(await page.locator('[data-slot="aui_user-message-root"]').filter({hasText:'Saved AB before'}).locator('[data-slot="inline-image"]').count(),1);
console.log('PASS both old stored message formats render as AB chips in the sent bubble');

} finally {await browser.close()}
