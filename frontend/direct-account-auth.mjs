const $=id=>document.getElementById(id);
const invoke=window.__TAURI__.core.invoke;
let busy=false,waiting=false,hasSecret=false,tokenReady=false,authorized=false;
function render(){
  $('save').disabled=busy||waiting;
  $('check-token').disabled=busy||waiting||!hasSecret;
  $('start').disabled=busy||waiting||!tokenReady;
  for(const id of ['open','poll','cancel'])$(id).disabled=busy||!waiting;
}
async function action(fn){if(busy)return;busy=true;$('error').textContent='';render();try{await fn();}catch(e){$('error').textContent=String(e);}finally{busy=false;render();}}
function confirmIdentity(){if(!$('identity-confirmed').checked)throw Error('请先核对飞书身份，并勾选保存凭据的确认项。');}
async function refresh(){const state=await invoke('direct_account_status');hasSecret=state.secretConfigured;authorized=state.authorized;$('saved-status').textContent=hasSecret?'新应用凭据已保存在独立 Windows 凭据库；未改动旧连接。':'尚未保存新应用密钥。';if(authorized)$('auth-status').textContent='独立会话已保存；尚未验证目标任务表或同步。';render();}
function progress(p){waiting=!p.done;$('auth-status').textContent=p.message+(p.userCode?` 验证码：${p.userCode}`:'')+(waiting?` 请间隔至少${p.retryAfterSeconds||5}秒，再检查结果。`:'');if(p.state==='authorized'){$('auth-status').textContent+=' 尚未验证目标任务表或同步，不会自动开始任务操作。';authorized=true;}}
$('save').onclick=()=>action(async()=>{confirmIdentity();const secret=$('app-secret').value;try{await invoke('direct_account_save_secret',{secret,confirmed:true});}finally{$('app-secret').value='';}tokenReady=false;await refresh();});
$('check-token').onclick=()=>action(async()=>{const result=await invoke('direct_account_check_token');tokenReady=result.usable===true;$('token-status').textContent=tokenReady?'应用令牌端点检查通过；未保存令牌、未调用任务接口。':`检查未通过，HTTP ${result.httpStatus}，业务码 ${result.code??'未知'}；未自动重试。`;});
$('start').onclick=()=>action(async()=>{confirmIdentity();progress(await invoke('local_start_isolated_feishu_auth'));});
$('open').onclick=()=>action(async()=>{await invoke('local_open_isolated_feishu_auth');$('auth-status').textContent+=' 已请求系统浏览器打开，请本人完成权限确认。';});
$('poll').onclick=()=>action(async()=>{progress(await invoke('local_poll_isolated_feishu_auth'));});
$('cancel').onclick=()=>action(async()=>{await invoke('local_cancel_isolated_feishu_auth');waiting=false;$('auth-status').textContent='已停止本机等待，不会撤销网页中已完成的授权。';});
action(refresh);
