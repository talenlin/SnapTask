// Manual, fixed-app reauthorization. Never performs task inspection or sync.
export function initIsolatedAuth({invoke,enabled,isBusy=()=>false,onStart=()=>{}}) {
  const $=id=>document.getElementById(id);
  const start=$('sync-reauthorize'),section=$('sync-auth'),message=$('sync-auth-status');
  const open=$('sync-auth-open'),poll=$('sync-auth-poll'),cancel=$('sync-auth-cancel');
  let waiting=false,busy=false;
  const render=()=>{start.hidden=!enabled();start.disabled=busy||waiting||isBusy();open.disabled=busy||!waiting;poll.disabled=busy||!waiting;cancel.disabled=busy||!waiting;};
  const progress=value=>{
    if(!value||typeof value.done!=='boolean')throw new Error('授权结果不完整，请停止等待后重试');
    waiting=!value.done;
    message.textContent=value.message+(value.userCode?` 验证码：${value.userCode}`:'')
      +(waiting?` 请等待至少${Math.max(1,value.retryAfterSeconds??5)}秒后点击“检查授权结果”。`:'')
      +(value.state==='authorized'?' 请点击“检查隔离测试表（只读）”核对实际身份与目标；不会自动同步。':'');
    if(value.done)open.hidden=true;
  };
  async function act(command) {
    if(!enabled()||isBusy()||busy)return;
    busy=true;section.hidden=false;render();
    try {
      if(command==='local_start_isolated_feishu_auth')onStart();
      const result=await invoke(command);
      if(command==='local_cancel_isolated_feishu_auth'){waiting=false;message.textContent='已停止本机等待；不会撤销你在飞书完成的授权。';}
      else if(command==='local_open_isolated_feishu_auth'){message.textContent='已请求系统浏览器打开飞书；请使用此应用所属的飞书身份自行确认，完成后返回检查授权结果。';}
      else {progress(result);open.hidden=result.done;}
    }catch(error){message.textContent=`${error}。未执行任务同步。`;}
    finally {busy=false;render();}
  }
  start.onclick=()=>{if(!waiting)return act('local_start_isolated_feishu_auth');};
  open.onclick=()=>{if(waiting)return act('local_open_isolated_feishu_auth');};
  poll.onclick=()=>{if(waiting)return act('local_poll_isolated_feishu_auth');};
  cancel.onclick=()=>{if(waiting)return act('local_cancel_isolated_feishu_auth');};
  return {refresh:render,isActive:()=>waiting||busy};
}
