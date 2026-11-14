export async function initMainFlowLab({invoke}) {
  const check=await invoke('local_main_flow_lab_status');
  if(check?.profile!=='app.snaptask.desktop.mainflow.rev36'||check.syntheticOnly!==true||check.networkEnabled!==false||check.credentialAccess!==false)throw Error('不是无网络的主流程验收环境');
  const node=(tag,text)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;return n;};
  document.querySelector('main').prepend(node('p','主流程隔离验收 rev36：全部目标和访问开关仅用于本机测试，不改变任何云端权限。无网络、无授权、无凭据读取；任务不会导入正式App。'));
  document.getElementById('backup-button').hidden=true;
  const host=node('section');host.className='sync-row';host.setAttribute('aria-label','本机主流程合成控制');
  host.append(node('strong','本机验收控制（不是真实连接设置）'));
  const diagnostic=node('p'),notice=node('p');diagnostic.setAttribute('role','status');notice.setAttribute('role','status');
  async function refresh(){const value=await invoke('local_main_flow_lab_status');diagnostic.textContent=`本机只读替身调用：${value.mockReads} 次；实际网络请求：0，凭据读取：0。`;}
  for(const [target,title] of [['a','验收：使用合成目标A'],['b','验收：切换合成目标B']]){
    const button=node('button',title);button.type='button';button.onclick=async()=>{
      button.disabled=true;try{await invoke('local_main_flow_lab_target',{target});await refresh();notice.textContent='只修改了本机合成目标；不会自动检查。请关闭并重开同步面板读取新目标。';}catch(error){notice.textContent=String(error);}finally{button.disabled=false;}
    };host.append(button);
  }
  const probe=node('button','验收：直接尝试后台只读守卫');probe.type='button';
  probe.onclick=async()=>{probe.disabled=true;try{
    const status=await invoke('get_local_feishu_access');
    await invoke('inspect_local_feishu_sync',{targetKey:status.targetKey});
  }catch(error){notice.textContent=String(error);}finally{await refresh();probe.disabled=false;}};
  const count=node('button','验收：刷新本机调用计数');count.type='button';count.onclick=refresh;
  host.append(probe,count,diagnostic,notice);document.getElementById('sync-cloud-hint').before(host);
  diagnostic.textContent=`本机只读替身调用：${check.mockReads} 次；实际网络请求：0，凭据读取：0。`;
}
