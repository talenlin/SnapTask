import {SHORTCUT_ACTIONS,defaultBindings,displayShortcut,eventChord,validateBindings,readBindings,writeBindings,exportShortcutText,parseShortcutText} from './local-workbench-shortcuts.mjs';

// This WebView's storage is profile-local and separate from task JSON backups.
export function initShortcutSettings({canOpen,onChange}) {
  const $=id=>document.getElementById(id), panel=$('shortcut-panel');
  const storage={getItem:key=>window.localStorage.getItem(key),setItem:(key,value)=>window.localStorage.setItem(key,value)};
  const initial=readBindings(storage);
  let bindings=initial.bindings,draft=null,recording=null,candidate=null;
  function status(text,error=false) { $('shortcut-status').textContent=text;$('shortcut-status').dataset.error=String(error); }
  function annotate() {
    $('settings-button').title='工作台设置';
    $('settings-button').removeAttribute('aria-keyshortcuts');
    for(const [id,action] of Object.entries(SHORTCUT_ACTIONS)) {
      const node=action.view?document.querySelector(`[data-view="${action.view}"]`):$(id),key=bindings[id];
      if(key)node.setAttribute('aria-keyshortcuts',displayShortcut(key).replace('Ctrl','Control'));else node.removeAttribute('aria-keyshortcuts');
      const label=id==='undo'?null:SHORTCUT_ACTIONS[id].label;
      if(label)node.title=`${label}${key?`（${displayShortcut(key)}）`:''}`;
    }
  }
  function stopRecording() { recording=null;candidate=null;$('shortcut-recorder').hidden=true;$('shortcut-use').disabled=true; }
  function render() {
    $('shortcut-rows').replaceChildren();
    let group=null;
    for(const [id,action] of Object.entries(SHORTCUT_ACTIONS)) {
      const nextGroup=action.view?'视图切换':'任务操作';
      if(group!==nextGroup){group=nextGroup;const heading=document.createElement('h3');heading.className='shortcut-group-title';heading.textContent=group;$('shortcut-rows').append(heading);}
      const row=document.createElement('section');row.className='shortcut-row';row.dataset.action=id;
      const text=document.createElement('div'),name=document.createElement('h3'),hint=document.createElement('p');name.textContent=action.label;hint.textContent=action.hint;hint.className='hint';text.append(name,hint);
      const key=document.createElement('kbd');key.textContent=displayShortcut(draft[id]);key.setAttribute('aria-label',`${action.label}：${key.textContent}`);
      const buttons=document.createElement('div');buttons.className='shortcut-row-actions';
      for(const [label,callback] of [
        ['修改',()=>{recording=id;candidate=null;$('shortcut-recorder').hidden=false;$('shortcut-record-label').textContent=`为「${action.label}」按下新组合键`;$('shortcut-record').value=action.view?'请按 Ctrl 加数字 1–5、字母或逗号':'请按 Ctrl 加字母、逗号或 Enter';$('shortcut-use').disabled=true;status('录入中，业务快捷键已暂停；Esc 取消录入。');$('shortcut-record').focus();}],
        ['清除',()=>{stopRecording();draft[id]=null;render();status('已清除草稿中的快捷键；点击“保存设置”生效。');focusRow(id);}],
        ['恢复默认',()=>{stopRecording();const proposed={...draft,[id]:action.defaultKey},error=validateBindings(proposed);if(error){status(error,true);return;}draft=proposed;render();status('已恢复此项默认键；点击“保存设置”生效。');focusRow(id);}],
      ]) {const b=document.createElement('button');b.type='button';b.textContent=label;b.setAttribute('aria-label',`${label}：${action.label}`);b.onclick=callback;buttons.append(b);}
      row.append(text,key,buttons);$('shortcut-rows').append(row);
    }
  }
  function focusRow(id) {panel.querySelector(`[data-action="${id}"] button`)?.focus();}
  function open() {if(!canOpen())return;draft={...bindings};stopRecording();render();status(initial.warning||'改键只影响本地工作台，不修改任务数据。');panel.showModal();$('shortcut-close').focus();}
  function close() {
    if(recording){const id=recording;stopRecording();status('已取消录入，原快捷键没有改变。');focusRow(id);return;}
    if(JSON.stringify(draft)!==JSON.stringify(bindings)&&!window.confirm('放弃尚未保存的快捷键设置？'))return;
    panel.close();$('settings-button').focus();
  }
  $('settings-button').onclick=open;$('shortcut-close').onclick=close;$('shortcut-cancel').onclick=close;
  $('shortcut-export-text').onclick=()=>{stopRecording();$('shortcut-transfer').value=exportShortcutText(bindings);$('shortcut-transfer').focus();$('shortcut-transfer').select?.();status('已显示当前已保存配置；可自行复制保存。未写入剪贴板或任务备份。');};
  $('shortcut-preview-text').onclick=()=>{
    try{const imported=parseShortcutText($('shortcut-transfer').value);stopRecording();draft=imported;render();status('配置已校验并载入草稿；核对后点击保存设置。当前快捷键尚未改变。');}
    catch(error){status(String(error),true);}
  };
  panel.addEventListener('cancel',e=>{e.preventDefault();close();});
  $('shortcut-use').onclick=()=>{
    if(!recording||!candidate)return;
    const id=recording,proposed={...draft,[id]:candidate},error=validateBindings(proposed);
    if(error){status(error,true);return;}
    draft=proposed;stopRecording();render();status('组合键已写入草稿；点击“保存设置”生效。');focusRow(id);
  };
  $('shortcut-record-cancel').onclick=()=>{const id=recording;stopRecording();status('已取消录入。');if(id)focusRow(id);};
  $('shortcut-defaults').onclick=()=>{if(!window.confirm('恢复全部默认快捷键？点击“保存设置”后生效，不会修改任务。'))return;stopRecording();draft=defaultBindings();render();status('已恢复默认草稿；请保存设置。');$('shortcut-defaults').focus();};
  $('shortcut-save').onclick=()=>{
    if(recording){status('请先使用或取消正在录入的组合键。',true);return;}
    try {writeBindings(storage,draft);bindings={...draft};initial.warning=null;annotate();onChange();status('快捷键设置已保存；重启后仍会保留。');}
    catch {status('快捷键设置保存失败。请检查当前电脑的存储权限后重试；本次未切换正在使用的快捷键。',true);}
  };
  document.addEventListener('keydown',e=>{
    if(!panel.open||!recording)return;
    if(e.key==='Tab')return;
    e.stopImmediatePropagation();
    if(e.key==='Escape'){e.preventDefault();const id=recording;stopRecording();status('已取消录入。');focusRow(id);return;}
    const chord=eventChord(e);
    e.preventDefault();
    if(['Control','Shift','Alt','Meta'].includes(e.key)||e.repeat||e.isComposing||e.keyCode===229)return;
    candidate=chord;
    const error=chord?validateBindings({...draft,[recording]:chord}):'请选择 Ctrl 加字母、逗号、数字 1–5 或 Enter；不支持 Alt、系统键或基础导航键。';
    $('shortcut-record').value=chord?displayShortcut(chord):'未识别为可用组合键';$('shortcut-use').disabled=!!error;
    status(error||'组合键可用；点击“使用此组合键”，再保存设置。',!!error);
  },true);
  window.addEventListener('blur',()=>{if(recording){stopRecording();status('窗口已失焦，录入已取消；请重新点击修改。');}});
  annotate();
  return {bindings:()=>bindings,recording:()=>!!recording,warning:initial.warning};
}
