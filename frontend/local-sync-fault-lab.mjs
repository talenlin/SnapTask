// Visible controls for the dedicated native lab. No network, auth, or cloud mutation.
export async function initFaultLab({invoke}) {
  const lab=await invoke('local_fault_lab_status');
  if(lab?.profile!=='app.snaptask.desktop.faultlab.rev34'||lab.synthetic!==true||lab.networkEnabled!==false||lab.cloudWritingEnabled!==false)throw new Error('非本机合成验收程序');
  const node=(tag,text)=>{const n=document.createElement(tag);if(text)n.textContent=text;return n;};
  const banner=node('p','本机合成验收 rev34：不连接飞书、不使用授权、不允许上传。每次启动创建新测试库，旧库保留。');
  banner.setAttribute('role','note');document.querySelector('main').prepend(banner);
  document.getElementById('backup-button').hidden=true;
  const panel=node('section');panel.className='sync-row';panel.setAttribute('aria-label','本机合成故障控制');
  panel.append(node('strong','仅本机验收控制（不是账号或权限设置）'));
  const select=node('select');select.setAttribute('aria-label','本机合成故障场景');
  for(const [value,text] of [['ok','正常合成结果'],['offline','模拟断网'],['expired','模拟授权过期'],['forbidden','模拟读取被拒绝 91403']]){const option=node('option',text);option.value=value;select.append(option);}
  const message=node('p');message.setAttribute('role','status');
  select.onchange=async()=>{select.disabled=true;try{await invoke('local_fault_lab_mode',{mode:select.value});message.textContent='本机场景已切换；请点击“检查隔离测试表（只读）”，不会自动检查。';}catch(e){message.textContent=String(e);}finally{select.disabled=false;}};
  const actions=node('div');actions.className='dialog-actions';
  for(const [target,text] of [['local','验收：修改未开始本地条目为71%'],['cloud','验收：改变未开始合成云端版本']]) {
    const button=node('button',text);button.type='button';
    button.onclick=async()=>{button.disabled=true;try{await invoke('local_fault_lab_change_remaining',{target});message.textContent='仅本机未开始条目的版本已变化。点击“验收：放行下一条”检查阻断；未改真实云端。';}catch(e){message.textContent=String(e);}finally{button.disabled=false;}};
    actions.append(button);
  }
  panel.append(select,node('p','版本变化按钮仅在两条下载批次已完成1/2、第二条尚未开始时可用。不要切换真实账号或权限。'),actions,message);
  document.getElementById('sync-cloud-hint').before(panel);
}
