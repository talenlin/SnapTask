// Local metadata and explicit read permission only; never invokes a cloud inspector.
export function initCloudSettings({container,invoke,onPermission,onInvalidate}) {
  const node=(tag,text)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;return n;};
  const title=node('h3','当前连接'),identity=node('p'),mode=node('p'),notice=node('p'),button=node('button');
  button.type='button';button.disabled=true;container.setAttribute('aria-label','工作台云端访问');
  const setup=node('button','设置飞书连接（主 App）'),target=node('a','打开当前目标表（飞书）');
  setup.type='button';target.hidden=true;target.setAttribute('target','_blank');target.setAttribute('rel','noopener noreferrer');
  setup.setAttribute('data-coach','cloud-setup');button.setAttribute('data-coach','cloud-access');
  const technical=node('details'),technicalTitle=node('summary','查看连接编号'),technicalText=node('p');
  technical.append(technicalTitle,technicalText);
  container.append(title,identity,mode,button,setup,target,technical,notice);
  let current=null,busy=false,generation=0;
  function clear(){current=null;target.hidden=true;target.setAttribute('href','');technicalText.textContent='';identity.textContent='';mode.textContent='云端访问状态尚未核实；本地任务仍可使用。';button.textContent='连接状态未核实';button.disabled=true;onPermission(false);}
  function render(value) {
    if(typeof value?.enabled!=='boolean'||typeof value?.configured!=='boolean'||typeof value.writeEnabled!=='boolean'||value.accountVerified!==false||typeof value.targetKey!=='string')throw Error('连接状态未核实，请重新打开面板');
    current=value;
    const safeTarget=value.configured&&/^[A-Za-z0-9]+$/.test(value.appToken||'')&&/^[A-Za-z0-9]+$/.test(value.tableId||'');
    target.hidden=!safeTarget;
    target.setAttribute('href',safeTarget?`https://my.feishu.cn/base/${value.appToken}?table=${value.tableId}`:'');
    identity.textContent=value.configured?'已保存一张飞书多维表格；账号尚未核实，请手动只读检查。':'尚未配置飞书连接；本地任务可继续使用。';
    mode.textContent=`云端访问${value.enabled?'已开启':'已关闭'} · ${value.enabled?'可读取并逐条导入本地':'需开启后才能读取'}。${value.writeEnabled?'上传需单独确认。':'当前只读。'}`;
    technicalText.textContent=value.configured?`应用 ${value.appId} · 多维表格 ${value.appToken} · 数据表 ${value.tableId}`:'';
    button.textContent=value.enabled?'关闭工作台云端访问':'开启工作台云端访问';button.disabled=!value.configured||busy;
    notice.textContent='这里的开关仅允许工作台访问飞书，不会自动读取或同步。';
    onPermission(value.configured&&value.enabled);
  }
  async function refresh() {
    if(busy)return;
    const version=++generation;clear();button.disabled=true;identity.textContent='正在读取本地连接设置（不联网）…';
    try{const value=await invoke('get_local_feishu_access');if(version===generation)render(value);}
    catch(error){if(version===generation){clear();notice.textContent=String(error);}}
  }
  setup.onclick=async()=>{
    if(busy)return;
    busy=true;setup.disabled=true;
    try{await invoke('open_local_feishu_setup');notice.textContent='已请求主 App 打开连接向导；不会自动授权、复制模板或同步任务。';}
    catch(error){notice.textContent=`无法打开连接向导：${error}。本地任务仍可使用。`;}
    finally{busy=false;setup.disabled=false;}
  };
  target.onclick=async event=>{
    event.preventDefault();
    if(target.hidden||!current?.configured)return;
    const url=target.href;
    if(!/^https:\/\/my\.feishu\.cn\/base\/[A-Za-z0-9]+\?table=[A-Za-z0-9]+$/.test(url))return;
    try{await window.__TAURI__.opener.openUrl(url);}
    catch(error){notice.textContent=`无法打开目标表：${error}。请在主 App 连接向导中核对连接。`;}
  };
  button.onclick=async()=>{
    if(busy||!current?.configured)return;
    busy=true;button.disabled=true;const target=current.targetKey,enabled=!current.enabled,version=++generation;
    onInvalidate();clear();notice.textContent='正在保存访问开关；关闭会等待已开始的请求完成，不能撤回已发出的提交。';
    try{const value=await invoke('set_local_feishu_access',{targetKey:target,enabled});if(version===generation){busy=false;render(value);}}
    catch(error){if(version===generation){clear();notice.textContent=`${error}；请关闭后重新打开面板核实状态。`;}}
    finally{busy=false;}
  };
  return {refresh,targetKey:()=>current?.targetKey,canInspect:()=>!busy&&current?.configured===true&&current.enabled===true,
    canUpload:()=>!busy&&current?.configured===true&&current.enabled===true&&current.writeEnabled===true,cancel(){generation++;clear();}};
}
