import { QUADRANTS, MOVE_STATUSES, moveProposal, statusOf, localDate, isOverdue, selectTasks, metrics, calendarMonth, shiftMonth } from './local-workbench-model.mjs';
import { SHORTCUT_ACTIONS, isTextEditingTarget, shortcutAction, displayShortcut } from './local-workbench-shortcuts.mjs';
import { initShortcutSettings } from './local-workbench-shortcut-settings.mjs';
import { initSyncPreview, formatContentTime } from './local-workbench-sync.mjs';
import { initWorkbenchGuide } from './local-workbench-guide.mjs';
import { initImeApi } from './local-workbench-ime.mjs';
const invoke = window.__TAURI__?.core?.invoke;
const $ = id => document.getElementById(id);
const faultLab=new URLSearchParams(location.search).get('faultLab')==='1';
if(new URLSearchParams(location.search).get('mainFlowLab')==='1')import('./local-main-flow-lab.mjs').then(m=>m.initMainFlowLab({invoke})).catch(error=>{$('message').textContent=String(error);});
if(faultLab)import('./local-sync-fault-lab.mjs').then(m=>m.initFaultLab({invoke})).catch(()=>{});
const form = $('task-form');
let snapshot = { tasks:[], undoLabel:null }, view = 'list', busy = false, loaded = false, editing = null, initialForm = '';
let messageTimer=null;
let restorePreview = null;
let dragging=null, pointerGesture=null, pendingMove=null, suppressClickUntil=0;
let calendarCursor=localDate().slice(0,7);
const viewMeta={list:'任务列表',board:'状态看板',quadrants:'四象限',calendar:'计划日历',stats:'统计'};
// One 24px line-icon vocabulary for functional navigation, not decorative imagery.
const icons={list:'M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01',board:'M3 4h5v16H3zM10 4h5v11h-5zM17 4h4v7h-4z',quadrants:'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z',calendar:'M8 2v4M16 2v4M3 10h18M3 4h18v18H3zM8 14h.01M12 14h.01M16 14h.01M8 18h.01M12 18h.01',stats:'M4 20V10M12 20V4M20 20v-7',backup:'M4 4h16v16H4zM8 4v6h8V4M8 20v-6h8v6',undo:'M9 5 4 10l5 5M4 10h10a6 6 0 0 1 6 6',add:'M12 5v14M5 12h14'};
for(const button of document.querySelectorAll('[data-icon]')) {
  const svg=document.createElementNS('http://www.w3.org/2000/svg','svg');svg.setAttribute('viewBox','0 0 24 24');svg.setAttribute('aria-hidden','true');svg.classList.add('icon');
  const path=document.createElementNS(svg.namespaceURI,'path');path.setAttribute('d',icons[button.dataset.icon]);svg.append(path);button.prepend(svg);
}
function el(tag, text, className) { const node=document.createElement(tag); if(text!==undefined) node.textContent=text; if(className) node.className=className; return node; }
function message(text,error=false) {
  const status=$('message');clearTimeout(messageTimer);status.textContent=text;
  status.dataset.error=String(error);status.dataset.success=String(!error&&loaded);
  status.dataset.passive=String(!error&&text==='本地任务已就绪');
  if(!error&&status.dataset.passive==='false')messageTimer=setTimeout(()=>{status.dataset.passive='true';},5000);
}
function setBusy(value) {
  if(value) clearDrag();
  busy=value; $('content').setAttribute('aria-busy',String(value));
  for(const id of ['add','undo','refresh','save','trash','cancel','cancel-top','backup-button','backup-export','backup-pick','backup-close','csv-export','settings-button']) $(id).disabled=value || (!loaded && ['add','undo','backup-button','backup-export','backup-pick','csv-export','settings-button'].includes(id)) || (id==='undo'&&!snapshot.undoLabel);
  $('sync-button').disabled=value||!loaded;
  $('guide-open').disabled=value;
  $('legacy-preview').disabled=value||!loaded;
  $('restore-choice').disabled=value;
  $('restore-confirm').disabled=value||!restorePreview||!$('restore-choice').checked;
  for(const field of form.querySelectorAll('input,textarea,select')) field.disabled=value;
  for(const control of document.querySelectorAll('#content button,#content input,#plan-summary button,[data-view],#search,#scope,#move-form input,#move-form button')) control.disabled=value;
  $('save').textContent=value?'正在保存…':'保存任务'; $('save').setAttribute('aria-busy',String(value));
}
async function operation(command,args,success,close=false) {
  if(busy) return;
  setBusy(true); $('form-error').hidden=true;
  try {
    if(!invoke) throw new Error('请从应用打开本地工作台；浏览器预览不保存任务。');
    const updated=await invoke(command,args);
    snapshot=command==='local_snapshot'?updated:{...updated,feishuReadonlyEnabled:snapshot.feishuReadonlyEnabled,feishuPairingSaveEnabled:snapshot.feishuPairingSaveEnabled}; loaded=true;
    $('storage-note').textContent=`${snapshot.demoMode?'隔离示例工作台；示例不是个人真实任务。':'本地工作台；不会自动导入或同步云端任务。'} 数据保存于：${snapshot.databasePath}。关闭后仍会保留。`;
    if(close==='move') { $('move-panel').close(); pendingMove=null; }
    else if(close) $('editor').close(); render(); message(success);
    if(command==='local_save_task')document.dispatchEvent(new Event('local-task-saved'));
    return true;
  } catch(e) {
    message(String(e),true);
    if($('editor').open) { $('form-error').textContent=String(e); $('form-error').hidden=false; }
    if($('move-panel').open) { $('move-error').textContent=String(e); $('move-error').hidden=false; }
    return false;
  } finally { setBusy(false); }
}
function progressNode(task) { const wrap=el('div',undefined,'progress'); const bar=el('progress'); bar.max=100; bar.value=task.progress; bar.setAttribute('aria-label',`完成进度 ${task.progress}%`); wrap.append(bar,el('span',`${task.progress}%`)); return wrap; }
function taskCard(task) {
  const button=el('button',undefined,'task-card'); button.type='button'; button.append(el('strong',task.title),progressNode(task));
  const meta=el('div',undefined,'meta'); meta.append(el('span',QUADRANTS[task.quadrant]));
  if(task.plannedDate) meta.append(el('span',`${isOverdue(task)?'逾期 · ':''}${task.plannedDate}`,isOverdue(task)?'overdue':''));
  if(task.today) meta.append(el('span','今日')); button.append(meta);
  button.dataset.taskId=task.id; button.draggable=false;
  button.setAttribute('aria-description','可拖动到其他分组，也可点击或按 Enter 编辑');
  button.onclick=e=>{if(e.detail===0||Date.now()>suppressClickUntil) openEditor(task);};
  button.onpointerdown=e=>{
    if(e.button!==0||!e.isPrimary||busy||!loaded||$('move-panel').open||pointerGesture) return;
    pointerGesture={pointerId:e.pointerId,x:e.clientX,y:e.clientY,view,task,button};
    button.setPointerCapture(e.pointerId);
  };
  button.onpointermove=e=>{if(beginPointerDrag(e)) {e.preventDefault();highlightLane(pointerLane(e));}};
  button.onpointerup=e=>{
    if(!pointerGesture||e.pointerId!==pointerGesture.pointerId) return;
    const started=beginPointerDrag(e), source=pointerGesture, lane=started?pointerLane(e):null;
    clearDrag();
    if(started) {
      e.preventDefault();suppressClickUntil=Date.now()+250;
      if(lane) requestMove(source.task,lane.dataset.kind,lane.dataset.target);
    }
  };
  button.onpointercancel=()=>{if(pointerGesture?.button===button) {if(dragging)suppressClickUntil=Date.now()+250;clearDrag();}};
  button.onlostpointercapture=()=>{if(pointerGesture?.button===button)clearDrag();};
  return button;
}
function beginPointerDrag(e) {
  if(!pointerGesture||e.pointerId!==pointerGesture.pointerId||pointerGesture.view!==view||busy) return false;
  if(!dragging&&Math.hypot(e.clientX-pointerGesture.x,e.clientY-pointerGesture.y)<6) return false;
  if(!dragging) {dragging=pointerGesture;pointerGesture.button.classList.add('dragging');}
  return true;
}
function pointerLane(e) {
  const lane=document.elementFromPoint(e.clientX,e.clientY)?.closest('.lane');
  const kind=view==='board'?'status':view==='quadrants'?'quadrant':null;
  return lane&&$('content').contains(lane)&&lane.dataset.kind===kind?lane:null;
}
function highlightLane(target) {
  for(const lane of document.querySelectorAll('.lane')) {
    const active=lane===target;lane.classList.toggle('drop-target',active);
    const hint=lane.querySelector('.drop-hint');if(hint)hint.hidden=!active;
  }
}
function clearDrag() {
  const gesture=pointerGesture;dragging=null;pointerGesture=null;
  if(gesture?.button.hasPointerCapture(gesture.pointerId))gesture.button.releasePointerCapture(gesture.pointerId);
  for(const node of document.querySelectorAll('.drop-target,.dragging')) node.classList.remove('drop-target','dragging');
  for(const hint of document.querySelectorAll('.drop-hint')) hint.hidden=true;
}
function dropLane(lane,kind,target) {
  lane.dataset.target=target; lane.dataset.kind=kind;
  const hint=el('p','松开以移动到此分组','drop-hint');hint.hidden=true;lane.append(hint);
}
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&pointerGesture) {e.preventDefault();suppressClickUntil=Date.now()+250;clearDrag();}});
const shortcuts=initShortcutSettings({
  canOpen:()=>loaded&&!busy&&!pointerGesture&&!document.querySelector('dialog[open]'),
  onChange:()=>updateUndoHint(),
});
const syncPreview=initSyncPreview({
  invoke,
  listen:window.__TAURI__?.event?.listen,
  canOpen:()=>loaded&&!busy&&!pointerGesture&&!document.querySelector('dialog[open]'),
  isDemo:()=>snapshot.demoMode,
  canInspectCloud:()=>snapshot.feishuReadonlyEnabled===true,
  canLinkPairing:()=>!snapshot.demoMode&&snapshot.feishuReadonlyEnabled===true,
  canDownload:()=>!snapshot.demoMode&&snapshot.feishuReadonlyEnabled===true,
  canUpload:()=>!snapshot.demoMode&&snapshot.feishuReadonlyEnabled===true,
  canReconcile:()=>!snapshot.demoMode&&snapshot.feishuReadonlyEnabled===true,
  canRestore:()=>!snapshot.demoMode&&snapshot.feishuReadonlyEnabled===true,
  canBatch:()=>!snapshot.demoMode&&snapshot.feishuReadonlyEnabled===true,
  canSavePairing:()=>snapshot.feishuPairingSaveEnabled===true,
  canAuthorize:()=>!faultLab,
  allowUpload:()=>!faultLab,
  inspectionLabel:faultLab?'本机合成检查（不是飞书账号）':'真实只读检查',
  onSynced:result=>{snapshot={...result,feishuReadonlyEnabled:snapshot.feishuReadonlyEnabled,feishuPairingSaveEnabled:snapshot.feishuPairingSaveEnabled};render();},
});
const imeApi=initImeApi({
  invoke,listen:window.__TAURI__?.event?.listen,
  getTasks:()=>snapshot.tasks,
  onChanged:async()=>{
    for(let attempt=0;busy&&attempt<20;attempt++)await new Promise(resolve=>setTimeout(resolve,250));
    if(busy)return;
    try{const updated=await invoke('local_snapshot');
      snapshot={...updated,feishuReadonlyEnabled:snapshot.feishuReadonlyEnabled,feishuPairingSaveEnabled:snapshot.feishuPairingSaveEnabled};
      render();}catch(error){message(String(error),true);}
  },
});
// Settings and browser authorization use other windows. Discard previews on focus loss,
// including late reads; regaining focus never initiates a cloud request.
window.addEventListener('blur',()=>{if(!snapshot.demoMode)syncPreview.invalidateConnection();});
function updateUndoHint() {
  const key=shortcuts.bindings().undo;
  $('undo').title=(snapshot.undoLabel?`撤销：${snapshot.undoLabel}`:'没有可撤销的操作')+(key?`（${displayShortcut(key)}）`:'（未设置快捷键）');
}
function switchView(next) {
  if(!loaded||busy||pointerGesture||document.querySelector('dialog[open]')||!Object.hasOwn(viewMeta,next))return;
  view=next;render();
}
document.addEventListener('keydown',e=>{
  const editingText=e.composedPath().some(isTextEditingTarget)||isTextEditingTarget(document.activeElement);
  const action=shortcutAction(e,shortcuts.bindings(),{busy,loaded,undoLabel:snapshot.undoLabel,dialogOpen:!!document.querySelector('dialog[open]'),editorOpen:$('editor').open,otherDialogOpen:!!document.querySelector('dialog[open]:not(#editor)'),dragging:!!pointerGesture,editingText,searchFocused:document.activeElement===$('search'),recording:shortcuts.recording()});
  if(!action)return;
  e.preventDefault();
  if(action==='undo')undoTask();
  else if(action==='add')openEditor();
  else if(action==='search'){$('search').focus();$('search').select();}
  else if(action==='save')form.requestSubmit($('save'));
  else if(SHORTCUT_ACTIONS[action].view){switchView(SHORTCUT_ACTIONS[action].view);document.querySelector(`[data-view="${view}"]`).focus();}
});
window.addEventListener('blur',clearDrag);
function requestMove(task,kind,target) {
  if(busy) return;
  const proposal=moveProposal(task,kind,target);
  if(!proposal.changed) {message('任务已在此分组；同组拖动不改变顺序或数据');return;}
  if(kind==='quadrant') {
    operation('local_move_task',{input:{id:task.id,expectedRevision:task.revision,kind,target,progress:null,confirmed:false}},'已移动四象限；进度和日期未改变，可撤销');return;
  }
  pendingMove={task,target};$('move-error').hidden=true;
  $('move-summary').textContent=`${task.title}：${statusOf(task.progress)}（${task.progress}%） → ${MOVE_STATUSES[target]}`;
  const input=$('move-progress');input.readOnly=target!=='doing';input.min=target==='doing'?1:proposal.progress;input.max=target==='doing'?99:proposal.progress;input.value=proposal.progress??'';
  $('move-hint').textContent=proposal.needsProgress?'请明确填写 1–99 的整数；不会自动猜测回退后的进度。':`确认后进度将为 ${proposal.progress}%。`;
  $('move-panel').showModal();(proposal.needsProgress?input:$('move-confirm')).focus();
}
function cancelMove() {if(busy)return;const id=pendingMove?.task.id;pendingMove=null;$('move-panel').close();for(const card of document.querySelectorAll('.task-card')) if(card.dataset.taskId===id) card.focus();}
$('move-cancel').onclick=cancelMove;$('move-close').onclick=cancelMove;
$('move-panel').addEventListener('cancel',e=>{e.preventDefault();cancelMove();});
$('move-form').onsubmit=e=>{
  e.preventDefault();if(busy||!pendingMove||!$('move-form').reportValidity())return;
  const progress=Number($('move-progress').value);
  if(!Number.isInteger(progress)) { $('move-error').textContent='请填写整数进度';$('move-error').hidden=false;return; }
  const {task,target}=pendingMove;
  operation('local_move_task',{input:{id:task.id,expectedRevision:task.revision,kind:'status',target,progress,confirmed:true}},'已移动状态；可以撤销','move');
};
function render() {
  document.body.dataset.activeView=view;document.body.dataset.scope=$('scope').value;
  $('workspace-date').textContent=new Date().toLocaleDateString('zh-CN',{year:'numeric',month:'long',day:'numeric',weekday:'long'});
  $('plan-summary').hidden=true;
  clearDrag();
  const root=$('content'); root.replaceChildren();
  $('view-title').textContent=viewMeta[view];
  $('work-area-title').textContent={list:'我的任务',board:'按状态查看',quadrants:'按四象限查看',calendar:'日历安排',stats:'完成情况'}[view];
  if(!loaded) { const empty=el('div',undefined,'empty'); empty.append(el('h2','尚未读取本地任务'),el('p','请刷新重试；读取成功后才显示任务和统计。')); root.append(empty); return; }
  const scope=$('scope').value;
  const tasks=selectTasks(snapshot.tasks,{ search:$('search').value, scope });
  if(view==='list'&&scope!=='trash')renderPlanSummary(tasks);
  const overview=metrics(snapshot.tasks);$('overview').replaceChildren(el('span','全部任务','overview-label'));
  for(const [label,value] of [['待推进',overview.total-overview.done],['今日待办',overview.today],['逾期未完成',overview.overdue],['已完成',overview.done]]) { const item=el('div',undefined,'overview-item');item.append(el('span',label),el('strong',String(value)));$('overview').append(item); }
  for(const button of document.querySelectorAll('[data-view]')) {
    if(button.dataset.view===view) button.setAttribute('aria-current','page'); else button.removeAttribute('aria-current');
  }
  updateUndoHint(); $('undo').disabled=busy||!snapshot.undoLabel;
  if(view==='calendar'&&scope!=='trash') {renderCalendar(root,tasks);return;}
  if(view==='stats' && scope!=='trash') {
    // Statistics share the exact visible filter, never just the first page of tasks.
    const m=metrics(tasks); const grid=el('div',undefined,'metric-grid');
    for(const [label,n] of [['任务总数',m.total],['已完成',m.done],['今日待办',m.today],['逾期未完成',m.overdue]]) { const card=el('div',undefined,'metric'); card.append(el('span',label),el('strong',String(n))); grid.append(card); }
    root.append(grid); const charts=el('div',undefined,'charts');
    for(const [title,rows] of [['状态分布',m.statuses],['四象限分布',m.quadrants]]) { const chart=el('section',undefined,'chart'); chart.append(el('h2',title)); for(const r of rows) { const line=el('div',undefined,'bar-row'); const bar=el('progress'); bar.max=Math.max(1,m.total); bar.value=r.count; bar.setAttribute('aria-label',`${r.label}：${r.count} 项`); line.append(el('span',r.label),bar,el('span',String(r.count))); chart.append(line); } charts.append(chart); }
    root.append(charts,el('p',`当前筛选：完成率 ${m.completionRate}%（已完成任务占比） · 平均进度 ${m.averageProgress}%`, 'rate')); return;
  }
  if(!tasks.length) {
    const empty=el('div',undefined,'empty'); empty.append(el('h2',scope==='trash'?'回收站为空':'这里还没有任务'),el('p',snapshot.tasks.length?'调整搜索或筛选范围试试。':'点击“新增任务”开始；不会自动导入云端记录。'));
    if(!snapshot.tasks.length&&snapshot.demoMode!==true&&scope!=='trash') {
      const notice=el('div',undefined,'empty-migration-notice'),open=el('button','升级后找不到原来的任务？打开备份与迁移');
      notice.id='empty-migration-notice';open.id='empty-migration-open';open.type='button';open.onclick=()=>{if(!busy&&loaded)$('backup-button').onclick();};notice.append(open);empty.append(notice);
    }
    root.append(empty); return;
  }
  if(view==='list'||scope==='trash') {
    const wrap=el('div',undefined,'table-wrap'), table=el('table'), head=el('thead'), row=el('tr');
    for(const h of ['任务名称','状态','进度','计划完成','四象限',...(scope==='trash'?['操作']:[])]) { const th=el('th',h); th.scope='col'; row.append(th); } head.append(row); table.append(head); const body=el('tbody');
    for(const task of tasks) {
      const row=el('tr'), title=el('td');
      if(scope==='trash') title.textContent=task.title; else { const b=el('button',task.title,'task-link'); b.type='button'; b.onclick=()=>openEditor(task); title.append(b);if(task.detail){const detail=el('p',task.detail,'task-row-detail');detail.title=task.detail;title.append(detail);} }
      const status=el('td'); status.append(el('span',statusOf(task.progress),`badge status-${task.progress===100?'done':task.progress===0?'todo':'doing'}`)); const progress=el('td'); progress.append(progressNode(task));
      row.append(title,status,progress,el('td',task.plannedDate||'未设置',isOverdue(task)?'overdue':''),el('td',QUADRANTS[task.quadrant]));
      if(scope==='trash') { const cell=el('td'), b=el('button','恢复'); b.onclick=()=>operation('local_set_trashed',{id:task.id,revision:task.revision,trashed:false},'已恢复任务'); cell.append(b); row.append(cell); } body.append(row);
    } table.append(body); wrap.append(table); root.append(wrap); return;
  }
  const board=el('div',undefined,view==='board'?'board':'board quadrants');
  const groups=view==='board'?Object.entries(MOVE_STATUSES).map(([key,label])=>[key,label,tasks.filter(t=>statusOf(t.progress)===label)]):Object.entries(QUADRANTS).map(([key,label])=>[key,label,tasks.filter(t=>t.quadrant===key)]);
  for(const [key,label,items] of groups) { const lane=el('section',undefined,'lane'), head=el('div',undefined,'lane-head'); head.append(el('h3',label),el('span',`${items.length} 项`)); lane.append(head);dropLane(lane,view==='board'?'status':'quadrant',key);for(const task of items) lane.append(taskCard(task)); if(!items.length) lane.append(el('p','暂无任务','lane-empty')); board.append(lane); }
  root.append(el('p',view==='board'?'拖动到其他状态后确认进度；也可点击或按 Enter 编辑。':'拖动到其他象限只改变分类；也可点击或按 Enter 编辑。','hint'),board);
}
function renderPlanSummary(tasks) {
  const root=$('plan-summary'),month=calendarMonth(calendarCursor);root.replaceChildren();root.hidden=false;
  const card=el('section',undefined,'plan-calendar'),head=el('header');
  const prev=el('button','‹'),next=el('button','›');prev.type=next.type='button';prev.setAttribute('aria-label','摘要日历上个月');next.setAttribute('aria-label','摘要日历下个月');
  prev.disabled=next.disabled=busy;
  prev.onclick=()=>{if(busy)return;calendarCursor=shiftMonth(calendarCursor,-1);render();};next.onclick=()=>{if(busy)return;calendarCursor=shiftMonth(calendarCursor,1);render();};
  head.append(prev,el('h2',month.label.replaceAll(' ','')),next);card.append(head);
  const week=el('div',undefined,'plan-week');week.setAttribute('aria-hidden','true');for(const name of '一二三四五六日')week.append(el('span',name));card.append(week);
  const days=el('div',undefined,'plan-days'),counts=new Map();for(const task of tasks)if(task.plannedDate)counts.set(task.plannedDate,(counts.get(task.plannedDate)||0)+1);
  for(const date of month.cells){const day=el('span',date?String(Number(date.slice(-2))):'');if(date){const count=counts.get(date)||0;day.setAttribute('aria-label',`${date}${date===localDate()?'，今天':''}，当前筛选 ${count} 项任务`);if(date===localDate())day.classList.add('today-date');if(count)day.classList.add('has-task');}days.append(day);}card.append(days);
  const open=el('button','查看此月计划 →','calendar-open');open.type='button';open.disabled=busy;open.onclick=()=>switchView('calendar');card.append(open);
  const m=metrics(tasks),summary=el('section',undefined,'plan-progress');summary.append(el('p','当前筛选 · 平均进度'));
  const value=el('strong',tasks.length?String(m.averageProgress):'—');if(tasks.length)value.append(el('small','%'));summary.append(value,el('span',`${m.total} 项任务 · ${m.done} 项已完成`));
  const bar=el('progress');bar.max=100;bar.value=m.averageProgress;bar.setAttribute('aria-label',tasks.length?`当前筛选平均进度 ${m.averageProgress}%`:'当前筛选暂无任务');summary.append(bar,el('p','本地保存，云端同步由你确认。'));root.append(card,summary);
}
function calendarTask(task) {
  const button=el('button',task.title,'calendar-task');button.type='button';button.dataset.taskId=task.id;button.dataset.status=task.progress===100?'done':task.progress===0?'todo':'doing';
  button.setAttribute('aria-label',`${task.title}，${task.progress}%，计划完成 ${task.plannedDate||'未设置'}`);button.onclick=()=>openEditor(task);return button;
}
function renderCalendar(root,tasks) {
  const month=calendarMonth(calendarCursor),bar=el('div',undefined,'calendar-toolbar');
  const controls=el('div',undefined,'actions');
  for(const [label,delta] of [['上个月',-1],['下个月',1]]) {const b=el('button',delta<0?'‹':'›');b.setAttribute('aria-label',label);b.onclick=()=>{calendarCursor=shiftMonth(calendarCursor,delta);render();};controls.append(b);}
  const today=el('button','本月');today.onclick=()=>{calendarCursor=localDate().slice(0,7);render();};controls.append(today);
  const jump=el('input');jump.type='month';jump.value=calendarCursor;jump.min='0001-01';jump.max='9999-12';jump.setAttribute('aria-label','选择月份');jump.onchange=()=>{if(jump.value&&jump.reportValidity()){calendarCursor=jump.value;render();}};
  bar.append(el('h2',month.label),controls,jump);root.append(bar);
  const dated=tasks.filter(t=>t.plannedDate?.startsWith(`${calendarCursor}-`)),undated=tasks.filter(t=>!t.plannedDate);
  root.append(el('p',`当前筛选：本月 ${dated.length} 项 · 未设日期 ${undated.length} 项 · 其他月份 ${tasks.length-dated.length-undated.length} 项。日期来自计划完成日期，不是开始日期。`,'hint calendar-note'));
  const grid=el('div',undefined,'calendar-grid');for(const name of ['周一','周二','周三','周四','周五','周六','周日']) grid.append(el('div',name,'calendar-weekday'));
  for(const date of month.cells) {
    const day=el('section',undefined,date?'calendar-day':'calendar-blank');
    if(date) {day.dataset.date=date;const items=dated.filter(t=>t.plannedDate===date);if(!items.length)day.classList.add('calendar-empty');if(date===localDate())day.classList.add('is-today');
      const number=el('h3',`${Number(date.slice(-2))}`,'calendar-number');number.setAttribute('aria-label',date);day.append(number);for(const task of items)day.append(calendarTask(task));}
    grid.append(day);
  }
  root.append(grid);
  const unscheduled=el('section',undefined,'unscheduled');unscheduled.append(el('h2',`未设置计划日期 · ${undated.length} 项`));
  if(undated.length) {for(const task of undated)unscheduled.append(calendarTask(task));}else unscheduled.append(el('p','当前筛选没有未设置日期的任务','hint'));root.append(unscheduled);
}
function formState() { return JSON.stringify(Object.fromEntries(new FormData(form))); }
function openEditor(task=null) {
  if(busy||!loaded||document.querySelector('dialog[open]')) return;
  editing=task; form.reset();
  void imeApi.showActivities(task?.id);
  $('content-time').textContent=task?`内容修改时间：${formatContentTime(task.contentModifiedAt)}。读取和无变化保存不更新时间；撤销属于一次新的修改。`:'保存后记录内容修改时间。';
  for(const name of ['title','detail','progress','quadrant','startDate','plannedDate']) form.elements[name].value=task?.[name] ?? (name==='progress'?0:name==='quadrant'?'in':'');
  form.elements.today.checked=task?.today??false;
  $('editor-title').textContent=task?'任务详情':'新增任务'; $('trash').hidden=!task; $('form-error').hidden=true; updateStatusHint(); initialForm=formState(); $('editor').showModal(); form.elements.title.focus();
}
function updateStatusHint() { const p=Number(form.elements.progress.value); $('status-hint').textContent=Number.isInteger(p)&&p>=0&&p<=100?statusOf(p):'请填写 0–100 的整数'; }
function cancelEditor() { if(busy) return; if(formState()!==initialForm&&!window.confirm('放弃尚未保存的修改？')) return; $('editor').close(); }
$('cancel').onclick=cancelEditor; $('cancel-top').onclick=cancelEditor; $('editor').addEventListener('cancel',e=>{ e.preventDefault(); cancelEditor(); });
form.elements.progress.addEventListener('input',updateStatusHint);
form.addEventListener('submit',e=>{
  e.preventDefault(); if(busy||!form.reportValidity()) return;
  const fields=Object.fromEntries(new FormData(form));
  operation('local_save_task',{input:{id:editing?.id??null,expectedRevision:editing?.revision??null,title:fields.title,detail:fields.detail,progress:Number(fields.progress),quadrant:fields.quadrant,today:form.elements.today.checked,startDate:fields.startDate||null,plannedDate:fields.plannedDate||null}},'任务已保存到本地',true);
});
$('trash').onclick=()=>{ if(!editing||busy) return; if(formState()!==initialForm&&!window.confirm('尚未保存的修改会放弃。将任务移入回收站？')) return; operation('local_set_trashed',{id:editing.id,revision:editing.revision,trashed:true},'已移入回收站，可以恢复或撤销',true); };
function undoTask() {
  if(!loaded||busy||!snapshot.undoLabel||pointerGesture||document.querySelector('dialog[open]')) return;
  return operation('local_undo',{},'已撤销上一步操作');
}
$('add').onclick=()=>openEditor(); $('undo').onclick=undoTask;
$('refresh').onclick=()=>operation('local_snapshot',{},'已读取本地任务');
$('search').oninput=render; $('scope').onchange=render;
for(const b of document.querySelectorAll('[data-view]')) b.onclick=()=>switchView(b.dataset.view);
// Date filters use local calendar days. Refresh at the next action after midnight.
let renderedDay=localDate(); window.addEventListener('focus',()=>{ if(localDate()!==renderedDay) {renderedDay=localDate();render();} });
let guideStorage;try {guideStorage=window.localStorage;}catch {guideStorage=null;}
const guide=initWorkbenchGuide({
  storage:guideStorage,
  canOpen:()=>!busy&&!pointerGesture,
  onError:text=>message(text,true),
});
// Offer help after the first local read, never an automatic modal or cloud request.
operation('local_snapshot',{},`本地任务已就绪${shortcuts.warning?`；${shortcuts.warning}`:''}`).then(()=>guide.initialize());

function backupStatus(text,error=false) { $('backup-status').textContent=text; $('backup-status').dataset.error=String(error); }
function clearRestorePreview() { restorePreview=null; $('restore-preview').hidden=true; $('restore-choice').checked=false; $('restore-confirm').disabled=true; }
async function backupAction(action,file=null) {
  if(busy||!loaded) return;
  if(action==='confirm'&&(!restorePreview||!$('restore-choice').checked)) return;
  setBusy(true);
  try {
    if(action==='export') {
      const result=await invoke('local_export_backup');
      backupStatus(result?`已导出 ${result.activeCount} 项任务、${result.trashCount} 项回收站记录及 ${result.historyCount} 项历史：${result.path}`:'已取消导出，未创建备份。');
    } else if(action==='csv') {
      const result=await invoke('local_export_csv');backupStatus(result?`已导出全部 ${result.count} 项未删除任务：${result.path}。CSV 不是完整备份。`:'已取消 CSV 导出，任务没有改变。');
    } else if(action==='preview'||action==='legacy') {
      const previousId=restorePreview?.requestId;
      clearRestorePreview();
      if(previousId) await invoke('local_cancel_restore',{requestId:previousId});
      let preview;
      if(action==='legacy') preview=await invoke('local_preview_legacy_workspace');
      else {
        if(!file||file.size>16*1024*1024) throw new Error('请选择不超过 16 MiB 的工作台 JSON 备份');
        const json=new TextDecoder('utf-8',{fatal:true}).decode(await file.arrayBuffer());
        preview=await invoke('local_preview_restore',{json,source:file.name});
      }
      if(!preview) { backupStatus('未选择文件，当前数据没有改变。'); return; }
      restorePreview=preview;
      $('restore-source').textContent=`来源：${preview.source}；备份时间：${new Date(preview.createdAt*1000).toLocaleString('zh-CN')}`;
      $('restore-counts').textContent=`当前共 ${preview.currentCount} 项记录，将替换为 ${preview.activeCount} 项任务、${preview.trashCount} 项回收站记录及 ${preview.historyCount} 项历史。${preview.activeCount+preview.trashCount===0?'注意：此备份为空，恢复会清空当前任务。':''}`;
      $('restore-samples').replaceChildren(...preview.sampleTitles.map(t=>el('li',t)));
      $('restore-preview').hidden=false;
      backupStatus('预览完成；只有勾选确认并执行恢复才会替换数据。');
    } else if(action==='confirm') {
      const result=await invoke('local_confirm_restore',{requestId:restorePreview.requestId,confirmReplace:true});
      snapshot={...result.snapshot,feishuReadonlyEnabled:snapshot.feishuReadonlyEnabled,feishuPairingSaveEnabled:snapshot.feishuPairingSaveEnabled}; clearRestorePreview(); render();
      $('storage-note').textContent=`数据保存于：${snapshot.databasePath}。当前数据的保护备份：${result.protectionPath}`;
      $('backup-panel').close(); message(`恢复完成；恢复前数据已保存：${result.protectionPath}`);
    }
  } catch(e) { backupStatus(String(e),true); message(String(e),true); }
  finally { setBusy(false); }
}
async function closeBackupPanel() {
  if(busy) return;
  setBusy(true);
  try { if(restorePreview) await invoke('local_cancel_restore',{requestId:restorePreview.requestId}); clearRestorePreview(); $('backup-panel').close(); }
  catch(e) {backupStatus(String(e),true);} finally {setBusy(false);}
}
$('backup-button').onclick=()=>{ if(!busy&&loaded) {backupStatus('');$('backup-panel').showModal();} };
$('backup-export').onclick=()=>backupAction('export'); $('backup-pick').onclick=()=>{if(!busy) {$('restore-file').value='';$('restore-file').click();}};
$('legacy-preview').onclick=()=>backupAction('legacy');
$('csv-export').onclick=()=>backupAction('csv');
$('restore-file').onchange=()=>{const file=$('restore-file').files[0];if(file) backupAction('preview',file);};
$('restore-confirm').onclick=()=>backupAction('confirm'); $('restore-choice').onchange=()=>{$('restore-confirm').disabled=busy||!restorePreview||!$('restore-choice').checked;};
$('backup-close').onclick=closeBackupPanel; $('backup-panel').addEventListener('cancel',e=>{e.preventDefault();closeBackupPanel();});
