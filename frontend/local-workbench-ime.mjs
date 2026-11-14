export function initImeApi({invoke,listen,getTasks,onChanged}) {
  const $=id=>document.getElementById(id);
  const status=(text,error=false)=>{$('ime-status').textContent=text;$('ime-status').dataset.error=String(error);};
  let enabled=false;
  async function refresh() {
    if(!invoke)return;
    try {
      const info=await invoke('local_ime_status');enabled=info.enabled;
      $('ime-toggle').textContent=enabled?'关闭接口':'开启接口';
      $('ime-address').textContent=info.address?`本机地址：${info.address}`:'接口未开启';
      $('ime-rotate').disabled=!enabled;
      status(info.error|| (enabled?'接口已开启；令牌只在生成时显示。':'接口关闭，输入法无法连接。'),!!info.error);
      await refreshReviews();
    } catch(error){status(String(error),true);}
  }
  function showToken(value) {
    $('ime-token-wrap').hidden=!value;
    $('ime-token').value=value||'';
    if(value)$('ime-token').select();
  }
  $('ime-toggle').onclick=async()=>{
    $('ime-toggle').disabled=true;showToken('');
    try {const result=await invoke('local_ime_set_enabled',{enabled:!enabled});enabled=result.enabled;
      $('ime-toggle').textContent=enabled?'关闭接口':'开启接口';$('ime-address').textContent=result.address?`本机地址：${result.address}`:'接口未开启';
      $('ime-rotate').disabled=!enabled;showToken(result.token);
      status(enabled?'接口已开启。将本机地址与连接令牌配置到输入法。':'接口已关闭。');
    } catch(error){status(String(error),true);}finally{$('ime-toggle').disabled=false;}
  };
  $('ime-rotate').onclick=async()=>{
    if(!window.confirm('重新生成令牌后，输入法中保存的旧令牌立即失效。继续吗？'))return;
    $('ime-rotate').disabled=true;showToken('');
    try{const result=await invoke('local_ime_rotate_token');showToken(result.token);status('新令牌已生成。请更新输入法设置。');}
    catch(error){status(String(error),true);}finally{$('ime-rotate').disabled=false;}
  };
  async function refreshReviews(){
    const wrap=$('ime-reviews');if(!invoke)return;
    try {
      const reviews=await invoke('local_ime_reviews');wrap.replaceChildren();
      if(!reviews.length){wrap.textContent='暂无待确认输入';return;}
      const tasks=getTasks().filter(t=>!t.trashed);
      for(const review of reviews){
        const card=document.createElement('section');card.className='shortcut-row';
        const text=document.createElement('p');text.textContent=review.text;card.append(text);
        const select=document.createElement('select');select.setAttribute('aria-label','选择目标任务');
        select.append(new Option('选择任务',''));
        for(const task of tasks)select.append(new Option(task.title,task.id));
        const progress=document.createElement('input');progress.type='number';progress.min='0';progress.max='100';progress.step='1';
        progress.value=review.progressAfter??'';progress.placeholder='总进度 0–100';progress.setAttribute('aria-label','完成后总进度');
        const confirm=document.createElement('button');confirm.type='button';confirm.textContent='确认写入';
        const cancel=document.createElement('button');cancel.type='button';cancel.textContent='取消这条';
        cancel.onclick=async()=>{
          if(!window.confirm('取消这条待确认输入？任务不会改变。'))return;
          cancel.disabled=true;
          try{await invoke('local_ime_cancel_review',{operationId:review.operationId});status('已取消这条输入。');await refreshReviews();}
          catch(error){status(String(error),true);cancel.disabled=false;}
        };
        confirm.onclick=async()=>{
          const value=Number(progress.value);
          if(!select.value||progress.value===''||!Number.isInteger(value)||value<0||value>100){status('请选择目标任务并填写 0–100 的总进度。',true);return;}
          if(!window.confirm(`确认将“${select.selectedOptions[0].textContent}”进度设为 ${value}% 并保存这条完成记录？`))return;
          confirm.disabled=true;
          try{await invoke('local_ime_confirm_review',{operationId:review.operationId,taskId:select.value,progress:value});
            status('已写入本地任务。');await onChanged();await refreshReviews();}
          catch(error){status(String(error),true);confirm.disabled=false;}
        };
        card.append(select,progress,confirm,cancel);wrap.append(card);
      }
    }catch(error){wrap.textContent=String(error);}
  }
  async function showActivities(taskId){
    const section=$('ime-activity-section'),list=$('ime-activity-list');section.hidden=!taskId;list.replaceChildren();
    if(!taskId||!invoke)return;
    try{const activities=await invoke('local_ime_activities',{taskId});
      if(!activities.length){section.hidden=true;return;}
      for(const activity of activities){const row=document.createElement('li');
        row.textContent=`${new Date(activity.createdAt).toLocaleString('zh-CN')} · ${activity.description}${activity.undone?'（已撤销）':''}`;list.append(row);}
    }catch{section.hidden=true;}
  }
  $('settings-button').addEventListener('click',()=>{if($('shortcut-panel').open)void refresh();});
  if(listen)void listen('local-ime-changed',()=>{void onChanged();if($('shortcut-panel').open)void refreshReviews();});
  return {showActivities,refreshReviews};
}
