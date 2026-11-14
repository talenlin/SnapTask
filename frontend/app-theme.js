// Appearance preferences only: no task storage, task commands or cloud access.
(() => {
  'use strict';
  const KEY='project-track.appearance.v1',valid=value=>value==='light'||value==='dark';
  // Windows owns the only outer clip. Browser previews retain their CSS rounding.
  if(window.__TAURI__?.window?.getCurrentWindow)document.documentElement.dataset.nativeFrame='true';
  let theme='dark',warning='';
  function read() {
    try {const saved=localStorage.getItem(KEY);warning=saved&&!valid(saved)?'原主题设置无效，已使用默认石墨蓝。':'';return valid(saved)?saved:'dark';}
    catch {warning='无法读取外观设置，暂用默认石墨蓝。任务数据不受影响。';return 'dark';}
  }
  function paint() {
    document.documentElement.dataset.theme=theme;
    // Match the native workbench title bar; CSS remains usable if the host lacks this API.
    const currentWindow=window.__TAURI__?.window?.getCurrentWindow?.();
    if(typeof currentWindow?.setTheme==='function')currentWindow.setTheme(theme).catch(()=>{});
    document.querySelectorAll('[data-theme-select]').forEach(node=>node.value=theme);
    document.querySelectorAll('[data-theme-toggle]').forEach(node=>{node.textContent=theme==='dark'?'切换浅色':'切换深色';node.setAttribute('aria-label',`切换为${theme==='dark'?'瓷白蓝浅色':'石墨蓝深色'}主题`);});
    document.querySelectorAll('[data-theme-status]').forEach(node=>{node.textContent=warning||`当前：${theme==='dark'?'石墨蓝（默认深色）':'瓷白蓝（可选浅色）'}；自动保存，仅影响外观。`;node.dataset.error=String(!!warning);});
    window.dispatchEvent(new CustomEvent('task-theme-changed',{detail:{theme}}));
  }
  function set(next) {
    if(!valid(next))return false;
    try {localStorage.setItem(KEY,next);theme=next;warning='';paint();return true;}
    catch {warning='外观设置保存失败，未切换主题；请检查本机存储权限。';paint();return false;}
  }
  theme=read();document.documentElement.dataset.theme=theme;
  window.TaskTheme=Object.freeze({get:()=>theme,set});
  function bind() {
    document.querySelectorAll('[data-theme-select]').forEach(node=>node.addEventListener('change',()=>set(node.value)));
    document.querySelectorAll('[data-theme-toggle]').forEach(node=>node.addEventListener('click',()=>set(theme==='dark'?'light':'dark')));
    paint();
  }
  window.addEventListener('storage',event=>{if(event.key===KEY||event.key===null){theme=read();paint();}});
  // Re-read when another WebView was closed/hidden during a preference change.
  window.addEventListener('focus',()=>{const current=read();if(current!==theme){theme=current;paint();}});
  if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',bind,{once:true});else bind();
})();
