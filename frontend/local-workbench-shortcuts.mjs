// Task undo must never steal text-editor undo or run behind a pending dialog.
export function isTextEditingTarget(node) {
  return !!(node?.isContentEditable || node?.closest?.('input,textarea,select,[role="textbox"]'));
}
export function shouldUndoTask(event, state) {
  return (event.code==='KeyZ'||event.key?.toLowerCase()==='z') && !!event.ctrlKey
    && !event.shiftKey && !event.altKey && !event.metaKey && !event.repeat
    && !event.isComposing && !event.defaultPrevented
    && state.loaded && !!state.undoLabel && !state.busy
    && !state.dialogOpen && !state.dragging && !state.editingText;
}

export const SHORTCUT_ACTIONS = Object.freeze({
  undo: {label:'撤销任务操作', defaultKey:'Ctrl+KeyZ', hint:'输入框内仍撤销文字；有弹窗时不撤销任务'},
  add: {label:'新增任务', defaultKey:'Ctrl+KeyN', hint:'只打开新增任务窗口，不立即保存'},
  search: {label:'搜索任务', defaultKey:'Ctrl+KeyF', hint:'主视图中定位任务搜索框'},
  save: {label:'保存当前任务', defaultKey:'Ctrl+Enter', hint:'仅任务编辑窗口；沿用全部填写校验'},
  viewList: {label:'任务列表',view:'list',defaultKey:'Ctrl+Digit1',hint:'主视图切换；保留搜索与筛选'},
  viewBoard: {label:'状态看板',view:'board',defaultKey:'Ctrl+Digit2',hint:'主视图切换；保留搜索与筛选'},
  viewQuadrants: {label:'四象限',view:'quadrants',defaultKey:'Ctrl+Digit3',hint:'主视图切换；保留搜索与筛选'},
  viewCalendar: {label:'计划日历',view:'calendar',defaultKey:'Ctrl+Digit4',hint:'主视图切换；保留搜索、筛选与查看月份'},
  viewStats: {label:'统计',view:'stats',defaultKey:'Ctrl+Digit5',hint:'主视图切换；保留搜索与筛选'},
});
export const SHORTCUT_STORAGE_KEY = 'local-workbench.shortcuts.v1';
export function exportShortcutText(bindings) {
  const error=validateBindings(bindings);if(error)throw Error(error);
  return JSON.stringify({format:'project-track-shortcuts',version:1,bindings},null,2);
}
export function parseShortcutText(text) {
  if(typeof text!=='string'||text.length>16384)throw Error('请选择不超过16 KiB的快捷键配置文本');
  let value;try{value=JSON.parse(text);}catch{throw Error('快捷键配置不是有效JSON');}
  if(value?.format!=='project-track-shortcuts'||value.version!==1||Object.keys(value).some(k=>!['format','version','bindings'].includes(k)))throw Error('快捷键配置格式或版本不支持');
  const error=validateBindings(value.bindings);if(error)throw Error(error);
  return {...value.bindings};
}
export function defaultBindings() {
  return Object.fromEntries(Object.entries(SHORTCUT_ACTIONS).map(([id,a])=>[id,a.defaultKey]));
}
export function displayShortcut(chord) {
  return chord ? chord.replace(/Key([A-Z])/g,'$1').replace(/Digit([1-5])/g,'$1').replace('Comma',',') : '未设置';
}
export function eventChord(event) {
  if(event.repeat||event.isComposing||event.keyCode===229||event.defaultPrevented||event.metaKey||event.altKey||!event.ctrlKey||event.getModifierState?.('AltGraph')) return null;
  const code=event.code||(/^[a-z]$/i.test(event.key??'')?`Key${event.key.toUpperCase()}`:/^[1-5]$/.test(event.key??'')?`Digit${event.key}`:event.shiftKey&&event.keyCode>=49&&event.keyCode<=53?`Digit${event.keyCode-48}`:event.key===','?'Comma':event.key);
  if(!/^(Key[A-Z]|Enter|Comma|Digit[1-5])$/.test(code??'')) return null;
  return `Ctrl+${event.shiftKey?'Shift+':''}${code}`;
}
export function validateBindings(bindings) {
  if(!bindings||typeof bindings!=='object'||Array.isArray(bindings)) return '快捷键配置格式无效';
  const seen=new Map();
  for(const [id,action] of Object.entries(SHORTCUT_ACTIONS)) {
    const chord=bindings[id];
    if(chord===null) continue;
    if(typeof chord!=='string'||!/^Ctrl\+(Shift\+)?(Key[A-Z]|Enter|Comma|Digit[1-5])$/.test(chord)) return `${action.label}：请选择 Ctrl 加字母、逗号、数字 1–5 或 Enter 的组合键`;
    const code=chord.split('+').at(-1);
    if(code.startsWith('Digit')&&!action.view) return '数字 1–5 组合键仅用于视图切换';
    const reserved=/^Key[ACVXYWTRLPOSQ]$/.test(code)||
      ['Ctrl+KeyB','Ctrl+KeyD','Ctrl+KeyE','Ctrl+KeyG','Ctrl+KeyH','Ctrl+KeyJ','Ctrl+KeyK','Ctrl+KeyU','Ctrl+Shift+KeyI','Ctrl+Shift+KeyJ','Ctrl+Shift+KeyZ','Ctrl+Shift+KeyN'].includes(chord);
    if(reserved) return `${displayShortcut(chord)} 已保留给文字编辑、系统或浏览器操作`;
    if(code==='KeyZ'&&id!=='undo') return 'Z 组合键保留给撤销任务；输入框内保留文字撤销';
    if(code==='Enter'&&id!=='save') return 'Enter 组合键只可用于保存当前任务';
    if(seen.has(chord)) return `${displayShortcut(chord)} 与「${SHORTCUT_ACTIONS[seen.get(chord)].label}」冲突，请先修改或清除该操作`;
    seen.set(chord,id);
  }
  if(Object.keys(bindings).some(id=>!Object.hasOwn(SHORTCUT_ACTIONS,id))) return '快捷键配置包含未知操作';
  return null;
}
export function readBindings(storage) {
  try {
    const raw=storage.getItem(SHORTCUT_STORAGE_KEY);
    if(raw===null) return {bindings:defaultBindings(),warning:null};
    const stored=JSON.parse(raw);
    if(stored.version!==1) throw Error('版本无效');
    if(!stored.bindings||typeof stored.bindings!=='object'||Array.isArray(stored.bindings))throw Error('格式无效');
    const bindings={...defaultBindings(),...stored.bindings};
    // Retired on request: keep the sidebar settings entry, not its shortcut.
    // Discard only this known legacy action, never other custom mappings.
    delete bindings.settings;
    const error=validateBindings(bindings);if(error)throw Error(error);
    return {bindings,warning:null};
  } catch {
    return {bindings:defaultBindings(),warning:'快捷键设置无法读取，当前使用安全默认值。可在设置中重新保存。'};
  }
}
export function writeBindings(storage,bindings) {
  const error=validateBindings(bindings);if(error)throw Error(error);
  const json=JSON.stringify({version:1,bindings});
  storage.setItem(SHORTCUT_STORAGE_KEY,json);
  if(storage.getItem(SHORTCUT_STORAGE_KEY)!==json)throw Error('快捷键设置未能读回，请重试');
}
export function shortcutAction(event,bindings,state) {
  const chord=eventChord(event);if(!chord||!state.loaded||state.busy||state.dragging||state.recording) return null;
  const id=Object.keys(SHORTCUT_ACTIONS).find(id=>bindings[id]===chord);if(!id) return null;
  if(id==='save') return state.editorOpen&&!state.otherDialogOpen?'save':null;
  if(state.dialogOpen) return null;
  if(id==='search') return !state.editingText||state.searchFocused?'search':null;
  if(state.editingText) return null;
  return id==='undo'&&!state.undoLabel?null:id;
}
