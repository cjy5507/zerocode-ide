import { openWindowTestPage } from "./window-boot.mjs";

// Runs against the real renderer with a local, deterministic native boundary.
// The corresponding Rust suite transfers actual bytes over SSH to OpenSSH.
export async function exerciseSftpAndTeam() {
window.__SFTP_CALLS__=[];
const fixtureEntry=(name,path,directory=false)=>({name,path,directory,symlink:false,size:directory?0:12345,modified:1788860000,permissions:directory?493:420});
window.__ANSWER__.sftp_jobs=()=>[];
window.__ANSWER__.sftp_list=({location})=>{window.__SFTP_CALLS__.push({command:'list',location});const path=location.path||(location.hostId?'/home/builder':'/Users/dev');return {path,parent:'/',entries:location.hostId?[fixtureEntry('releases',path+'/releases',true),fixtureEntry('config.yml',path+'/config.yml'),fixtureEntry('서버 로그.txt',path+'/서버 로그.txt'),fixtureEntry('.env',path+'/.env')]:[fixtureEntry('Projects',path+'/Projects',true),fixtureEntry('report.csv',path+'/report.csv'),fixtureEntry('deploy.sh',path+'/deploy.sh')]};};
window.__ANSWER__.sftp_enqueue=({spec})=>{window.__SFTP_CALLS__.push({command:'enqueue',spec});return {id:'job-'+window.__SFTP_CALLS__.length,spec,state:'queued',progress:{}};};
window.__ANSWER__.sftp_read_text=()=> 'hello';
window.__ANSWER__.sftp_write_text=(args)=>{window.__SFTP_CALLS__.push({command:'write',...args});return null;};
window.__ANSWER__.sftp_connect=()=>[{hostId:'fixture-host',state:'connected',home:'/home/builder'}];
window.__ANSWER__.sftp_connections=()=>[];
window.__ANSWER__.sftp_rename=(args)=>{window.__SFTP_CALLS__.push({command:'rename',...args});return null;};
window.__ANSWER__.sftp_mkdir=(args)=>{window.__SFTP_CALLS__.push({command:'mkdir',...args});return null;};
window.__ANSWER__.sftp_search=({location,query})=>{window.__SFTP_CALLS__.push({command:'search',location,query});return [];};

window.__ANSWER__.sftp_sources=()=>[{hostId:"fixture-host",label:"Build server",targetId:null,roots:["/home/builder"]}];

 await openSftpManager({id:'fixture-host'});
 const checks=[]; const ok=(name,condition,detail='')=>{checks.push({name,ok:!!condition,detail});if(!condition)throw new Error(name+': '+detail);};
 await new Promise(r=>setTimeout(r,300));
 ok('folder icons use the product symbol',document.querySelector('.sftp-files use')?.getAttribute('href')==='#i-folder');
 const panes=[...document.querySelectorAll('.sftp-pane')];
 const settle=(ms)=>new Promise(r=>setTimeout(r,ms));
 // A file manager shows the way up as an entry, not only as a toolbar button.
 const parentRows=panes.map(p=>p.querySelector('tbody tr:first-child'));
 ok('both panes open with a .. row that leads to the parent',parentRows.every(r=>r?.classList.contains('sftp-parent')&&r.querySelector('.sftp-entry-name')?.textContent==='..'&&!r.dataset.path),parentRows.map(r=>r?.outerHTML.slice(0,120)).join(' | '));
 const listCalls=()=>window.__SFTP_CALLS__.filter(c=>c.command==='list');
 const beforeUp=listCalls().length; parentRows[1].querySelector('button').click(); await settle(80);
 ok('activating .. lists the parent folder',listCalls().length===beforeUp+1&&listCalls().at(-1).location.path==='/'&&sftpPanes.remote.path==='/',JSON.stringify(listCalls().at(-1)));
 ok('.. is never part of the selection',sftpPanes.remote.selected.size===0&&![...document.querySelectorAll('.sftp-pane[data-side="remote"] tr[data-path] .sftp-entry-name')].some(n=>n.textContent==='..'));
 await sftpLoad('remote','/home/builder');
 // An empty search word is refused in the person's language, before any round trip.
 el('sftp-remote-filter').value=''; panes[1].querySelector('[data-action="search"]').click(); await settle(30);
 ok('an empty search asks for a word in the person\'s language',document.querySelector('.sftp-error').textContent===sftpWords.searchNeedsText()&&!window.__SFTP_CALLS__.some(c=>c.command==='search'),document.querySelector('.sftp-error').textContent);
 sftpError('');
 // The launcher stands beside the floating-workspace door, wherever that door was left.
 const wasHidden=sftpLauncher.hidden; sftpLauncher.hidden=false; paintFloatTrigger(); await settle(30);
 const doorBox=el('float-toggle').getBoundingClientRect(), launcherBox=sftpLauncher.getBoundingClientRect();
 ok('the SFTP launcher stands beside the floating-workspace door',Math.abs(launcherBox.top-doorBox.top)<1&&Math.abs(launcherBox.height-doorBox.height)<1&&launcherBox.right<=doorBox.left&&doorBox.left-launcherBox.right<24,JSON.stringify({doorBox,launcherBox}));
 sftpLauncher.hidden=wasHidden;
 ok('both file lists align',Math.abs(panes[0].querySelector('table').getBoundingClientRect().top-panes[1].querySelector('table').getBoundingClientRect().top)<2);
 const remoteRow=[...document.querySelectorAll('.sftp-pane[data-side="remote"] tr[data-path]')].find(r=>r.dataset.path.endsWith('config.yml'));
 remoteRow.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:600,clientY:300}));
 const labels=[...sidebarMenu.querySelectorAll('[role="menuitem"]')].map(n=>n.textContent);
 ok('right click offers real file operations',labels.includes(sftpWords.copy())&&labels.includes(sftpWords.chmod())&&labels.includes(sftpWords.remove()));
 const menuCopy=[...sidebarMenu.querySelectorAll('button')].find(n=>n.textContent===sftpWords.copy());menuCopy.click();await new Promise(r=>setTimeout(r,30));
 ok('context copy uses the selected remote file',sftpClipboard.entries[0].location.path==='/home/builder/config.yml');
 const entry=sftpPanes.local.entries.find(e=>e.name==='report.csv');
 sftpPanes.local.selected.add(entry.path);paintSftpSelection('local');
 const pendingTransfer=sftpTransfer('local','remote');
 await new Promise(r=>setTimeout(r,30));
 ok('transfer popup names the destination before queuing',document.querySelector('.sftp-transfer-target').textContent.includes('/home/builder')&&!window.__SFTP_CALLS__.some(c=>c.command==='enqueue'));
 document.querySelector('.sftp-transfer-confirm').click();await pendingTransfer;
 const call=window.__SFTP_CALLS__.filter(c=>c.command==='enqueue').at(-1);
 ok('upload routes the selected file across the correct host boundary',call.spec.source.hostId===null&&call.spec.destination.hostId==='fixture-host'&&call.spec.destination.path==='/home/builder/report.csv');
 const originalEnqueue=window.__ANSWER__.sftp_enqueue;
 const previousRemote={hostId:sftpPanes.remote.hostId,path:sftpPanes.remote.path};
 sftpPanes.local.selected=new Set(sftpPanes.local.entries.filter(e=>!e.directory).map(e=>e.path));sftpCopy('local',false);
 const batchStart=window.__SFTP_CALLS__.length;
 window.__ANSWER__.sftp_enqueue=(args)=>{sftpPanes.remote.hostId='other-host';sftpPanes.remote.path='/other';return originalEnqueue(args);};
 await sftpPaste('remote');
 const batch=window.__SFTP_CALLS__.slice(batchStart).filter(c=>c.command==='enqueue');
 ok('a batch cannot change destination when the selected host changes',batch.length===2&&batch.every(c=>c.spec.destination.hostId===previousRemote.hostId&&c.spec.destination.path.startsWith(previousRemote.path+'/')));
 Object.assign(sftpPanes.remote,previousRemote);window.__ANSWER__.sftp_enqueue=originalEnqueue;
 sftpPanes.remote.selected=new Set(['/home/builder/config.yml']);sftpCopy('remote',true);await sftpPaste('local');
 ok('cut and paste requests a verified move',window.__SFTP_CALLS__.filter(c=>c.command==='enqueue').at(-1).spec.operation==='move');
 await sftpEdit({hostId:'fixture-host',path:'/home/builder/config.yml'});
 const editor=document.querySelector('.sftp-editor');const box=editor.querySelector('textarea').getBoundingClientRect();const footer=editor.querySelector('footer').getBoundingClientRect();
 ok('editor body fills available height',box.height>400,box.height);
 ok('footer controls retain normal button height',editor.querySelector('footer button').getBoundingClientRect().height<50);
 editor.querySelector('textarea').value='updated';editor.querySelector('[data-action="save"]').click();await new Promise(r=>setTimeout(r,30));
 const write=window.__SFTP_CALLS__.filter(c=>c.command==='write').at(-1);ok('editor sends the original revision for conflict checking',write.expected==='hello'&&write.text==='updated');
 await sftpEditor.close();
 // Renderer-only terminal fixtures never start an agent or a process.
 closeSftpManager(); mountTermTab(9101,{agent:'ZO',worktree:activeWorktreePath},{placement:'tab'}); const main=tabs.find(t=>t.id===termTabId(9101));
 const emit=id=>{paneAgents.set(id,'zo');for(const f of window.__LISTENERS__['term:split']||[])f({payload:{parent:9101,term:id,direction:id===9102?'vertical':'horizontal',agent:'zo',helper:'fixture-'+id}});};
 emit(9102);emit(9103);equalizeActivePanes(main,{focus:false});await new Promise(r=>setTimeout(r,50));
 ok('main plus two workers keeps exactly three panes',paneLeaves(main.layout).length===3);
 const slots=[...paneHosts.get(main.id).querySelectorAll('.pane-slot')].map(n=>n.getBoundingClientRect().width);
 ok('three panes are equalized',Math.max(...slots)-Math.min(...slots)<2,slots);
 const before=window.__COUNTS__.close_term||0;emit(9104);await new Promise(r=>setTimeout(r,50));
 ok('fourth pane parks all workers and retains the main',JSON.stringify(paneLeaves(main.layout))==='[9101]'&&[9102,9103,9104].every(id=>detachedAgents.has(id)));
 revealListedWorker(9102,main.worktree,'zo');const viewer=tabs.find(t=>t.teamParent===9101);const viewerId=viewer.id;
 revealListedWorker(9103,main.worktree,'zo');ok('worker selection reuses one full-size viewer',tabs.filter(t=>t.teamParent===9101).length===1&&viewer.id===viewerId&&paneLeaves(viewer.layout)[0]===9103&&detachedAgents.has(9102));
 ok('layout transitions never close a running process',(window.__COUNTS__.close_term||0)===before);
 return checks;
}

export async function testSftpAndTeam(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const checks = await page.evaluate(exerciseSftpAndTeam);
    for (const check of checks) ok(check.name, check.ok, JSON.stringify(check.detail));
    ok("SFTP and worker switching have no renderer exceptions", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}
