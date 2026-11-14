// Explicit single-record synchronization; cloud writes never run from inspection.
import {renderBatchPreview,runBatchQueue} from './local-sync-batch.mjs';
import {initIsolatedAuth} from './local-isolated-auth.mjs';
import {initCloudSettings} from './local-workbench-cloud-settings.mjs';
export const SYNC_LABELS = Object.freeze({
  localChanged:'仅本地更新', cloudChanged:'仅飞书更新', conflict:'两边都有修改',
  manual:'需要人工判断', equal:'内容一致', unverified:'关联记录待核实',
  unlinked:'本地任务尚未关联', cloudUnlinked:'飞书任务尚未关联', deletionConflict:'删除／恢复冲突',
});
const FIELD_NAMES = {title:'名称',detail:'详情',progress:'进度',quadrant:'四象限',today:'今日待办',startDate:'开始日期',plannedDate:'计划完成日期'};
export function conflictWarning(comparison,direction='') {
  if(comparison.status!=='conflict')return '';
  const target=direction==='local'?'飞书':direction==='cloud'?'本地':'目标一侧';
  return `两边都有修改：整条覆盖会舍弃${target}相对同步基线的修改，不会自动合并；时间较晚也不代表包含另一侧修改。请核对全部字段后决定保留哪一侧。`;
}
export function formatContentTime(value) {
  return Number.isSafeInteger(value)&&value>0&&value<=253402300799999
    ? new Date(value).toLocaleString('zh-CN',{hour12:false}) : '历史时间未知';
}
export function formatDifference(field,value) {
  if(value==null||value==='') return '未设置';
  if(field==='progress') return `${value}%`;
  if(field==='today') return value?'是':'否';
  if(field==='quadrant') return ({iu:'重要且紧急',in:'重要不紧急',un:'紧急不重要',nn:'不重要不紧急'})[value]??String(value);
  return String(value);
}
export function assertDemo(result) {
  if(result?.simulated!==true||result?.preview?.readOnly!==true||!Array.isArray(result.preview.items)) throw new Error('未返回只读示例；本次不展示结果。');
  return result;
}
// Late responses cannot reopen a closed panel or replace a newer request.
export function createPreviewLoader({request,onLoading,onResult,onError}) {
  let generation=0;
  return {
    cancel(){generation++;},
    async load(command,validate=value=>value) {
      const current=++generation;onLoading();
      try {const result=validate(await request(command));if(current===generation)onResult(result);}
      catch(error) {if(current===generation)onError(String(error));}
    },
  };
}
export function initSyncPreview({invoke,listen,canOpen,isDemo,canInspectCloud=()=>false,canLinkPairing=()=>false,canDownload=()=>false,canUpload=()=>false,canReconcile=()=>false,canRestore=()=>false,canBatch=()=>false,canSavePairing=()=>false,onSynced=()=>{},canAuthorize=()=>true,allowUpload=()=>true,inspectionLabel='真实只读检查'}) {
  const $=id=>document.getElementById(id), panel=$('sync-panel');
  let savedNotice='',syncing=false,viewGeneration=0,activeBatchStop;
  const auth=initIsolatedAuth({invoke,enabled:()=>isDemo()&&canSavePairing()&&canAuthorize(),isBusy:()=>syncing,
    onStart:()=>{viewGeneration++;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-identity').textContent='';status('正在续接隔离测试应用授权；旧预览已清除，不执行同步。');}});
  const node=(tag,text,cls)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;if(cls)n.className=cls;return n;};
  const status=(text,error=false)=>{$('sync-status').textContent=text;$('sync-status').dataset.error=String(error);};
  const uploadNotice=result=>({completed:'飞书读回与本次提交内容一致，操作已核实。',pending:'写入结果待核实；请只读核实，不会自动重发。',notApplied:'写入尚未发送；请重新检查字段与权限后再决定。',conflict:'飞书内容与提交内容或原版本不一致；停止重发，请重新核对。',restored:'已恢复上传前的飞书内容。'})[result?.state]??'操作结果未知，请只读核实。';
  async function runBatch(start,formal) {
    if(syncing||auth.isActive()||(formal&&(formal.generation!==viewGeneration||!cloudSettings?.canInspect())))return;
    syncing=true;$('sync-close').disabled=true;$('sync-cloud-check').disabled=true;$('sync-example').disabled=true;
    let stopped=false,job,runId;
    const stop=node('button','停止后续条目（当前条目仍需核实）');stop.type='button';stop.disabled=true;$('sync-content').prepend(stop);
    // Explicit acceptance URL only, still gated by the isolated execution capability.
    const controlledPause=canSavePairing()&&new URLSearchParams(globalThis.location?.search??'').get('batchAcceptancePause')==='1';
    let releasePause;
    const pause=node('section',undefined,'sync-row');
    const stopBatch=async(notify=true)=>{
      stopped=true;stop.disabled=true;
      if(job)try{
        await invoke(formal?'stop_local_feishu_batch':'local_control_isolated_feishu_batch',formal?{targetKey:formal.targetKey,batchId:job.id}:{batchId:job.id,action:'stop'});
        if(notify)status('已请求停止；当前条目会完成核实，后续条目不再发出。');
      }catch(error){if(notify)status(`${error}；本窗口已停止调度，请重新检查批次。`,true);}
      finally{releasePause?.();}
    };
    stop.onclick=()=>stopBatch();if(formal)activeBatchStop=()=>stopBatch(false);
    try {
      job=await runBatchQueue({start:async()=>{
        const value=await start();
        if(formal){
          job=value;
          if(typeof value?.runId!=='string'||!value.runId)throw Error('批次运行许可不完整，请重新核对');
          runId=value.runId;
          if(stopped||formal.generation!==viewGeneration)await stopBatch(false);
        }
        return value;
      },stopped:()=>stopped||!!(formal&&formal.generation!==viewGeneration),beforeStep:async value=>{
        if(!controlledPause||value.completed===0)return;
        await new Promise(resolve=>{
          releasePause=resolve;
          const next=node('button','验收：放行下一条（仍会复查版本）');next.type='button';
          next.onclick=()=>{next.disabled=true;resolve();};
          pause.replaceChildren();pause.append(node('p',`受控验收暂停：${value.completed}/${value.total} 已核实，下一条尚未发出。可点击上方停止按钮。`),next);
          $('sync-content').prepend(pause);pause.prepend(stop);pause.scrollIntoView?.({block:'center'});
          status(`受控验收暂停 ${value.completed}/${value.total}；等待停止或人工放行，不自动继续。`);
        });
        releasePause=undefined;
      },step:id=>invoke(formal?'step_local_feishu_batch':'local_step_isolated_feishu_batch',formal?{targetKey:formal.targetKey,input:{contextVersion:formal.contextVersion,batchId:id,runId}}:{batchId:id}),onProgress:value=>{job=value;stop.disabled=stopped||value.state!=='ready';if(!formal||formal.generation===viewGeneration)status(`批次 ${value.completed}/${value.total} 已核实；逐条重新读取并复查版本…`);}});
      if(!formal||formal.generation===viewGeneration)savedNotice=`批次 ${job.completed}/${job.total}：${job.message}`;
    } catch(error){if(formal)await stopBatch(false);if(!formal||formal.generation===viewGeneration)savedNotice=`批次停止：${error}。请重新检查保存的批次与单条日志，不会自动重发。`;}
    finally {
      stop.disabled=true;activeBatchStop=undefined;
      try{
        if(formal&&formal.generation===viewGeneration){
          viewGeneration++;const finishedGeneration=viewGeneration;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-identity').textContent='';
          status(`${savedNotice} 请手动重新检查；旧确认已清除，不会自动继续。`);savedNotice='';
          try{const snapshot=await invoke('local_snapshot');if(finishedGeneration===viewGeneration)onSynced(snapshot);}
          catch(error){if(finishedGeneration===viewGeneration)status(`批次已停止调度，但本地显示刷新失败：${error}。请刷新任务并手动检查批次，不要重发。`,true);}
        }else if(!formal){onSynced(await invoke('local_snapshot'));await loader.load('local_inspect_isolated_feishu_pairing',validatePairing);}
      }finally{syncing=false;$('sync-close').disabled=false;$('sync-cloud-check').disabled=!canInspectCloud()||(!isDemo()&&!cloudSettings?.canInspect());$('sync-example').disabled=false;}
    }
  }
  const batchInput=plan=>({entries:plan.items.map(({row,direction})=>({direction,input:{localId:row.localId,cloudId:row.cloudId,subjectId:row.subjectId,expectedLocalRevision:row.localRevision,expectedLocalModifiedAt:row.localModifiedAt,expectedCloudModifiedAt:row.cloudModifiedAt,expectedCloudFields:row.expectedCloudFields}}))});
  async function uploadAction(command,args) {
    if(syncing||auth.isActive())return;
    syncing=true;$('sync-close').disabled=true;$('sync-cloud-check').disabled=true;$('sync-example').disabled=true;
    status(command.includes('reconcile')?'正在只读核实，不重发…':'正在复查版本、单次提交并读回核实…');
    try {savedNotice=uploadNotice(await invoke(command,args));}
    catch(error){savedNotice=`${error}。请检查操作记录；不会自动重发。`;}
    try {await loader.load('local_inspect_isolated_feishu_pairing',validatePairing);}
    finally {syncing=false;$('sync-close').disabled=false;$('sync-cloud-check').disabled=false;$('sync-example').disabled=false;}
  }
  async function formalUploadAction(command,args,generation) {
    if(syncing||auth.isActive()||generation!==viewGeneration||!cloudSettings?.canInspect())return;
    syncing=true;$('sync-close').disabled=true;$('sync-cloud-check').disabled=true;$('sync-example').disabled=true;
    status(command==='reconcile_local_feishu_upload'?'正在只读核实，没有重新发送写入…':command==='restore_local_feishu_upload'?'正在复查上传后版本，准备单次恢复…':'正在复查版本、记录待核实意图并单次提交…');
    try {
      const result=await invoke(command,args);
      if(generation!==viewGeneration)return;
      if(!result?.id||!['completed','pending','conflict','notApplied','restored'].includes(result.state))throw Error('操作返回结果不完整，请核对操作记录');
      viewGeneration++;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-identity').textContent='';
      const restoring=result.isRestore===true||command==='restore_local_feishu_upload';
      const notice=restoring?({completed:'恢复上传前内容已读回核实；本地任务没有修改。',pending:'恢复结果待核实；请只读核实，不要重复恢复。',conflict:'恢复结果与预期不一致，请核实；不会自动重发。',notApplied:'恢复尚未发送，请重新检查。'})[result.state]??uploadNotice(result):uploadNotice(result);
      status(`${notice} 请手动重新检查查看操作记录；不会自动同步或重发。`);
    }catch(error){
      if(generation===viewGeneration){viewGeneration++;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-identity').textContent='';status(`${error}。请手动检查操作记录核实结果，不要重发。`,true);}
    }finally{syncing=false;$('sync-close').disabled=false;$('sync-example').disabled=false;$('sync-cloud-check').disabled=!canInspectCloud()||!cloudSettings?.canInspect();}
  }
  const loader=createPreviewLoader({
    request:command=>{if(!invoke)throw new Error('请从应用打开工作台。');return command==='inspect_local_feishu_sync'?invoke(command,{targetKey:cloudSettings?.targetKey()}):invoke(command);},
    onLoading:()=>{viewGeneration++;$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-advanced').hidden=true;$('sync-identity').textContent='';$('sync-content').setAttribute('aria-busy','true');status('正在读取飞书任务，不会修改内容…');},
    onError:error=>{$('sync-content').setAttribute('aria-busy','false');status(`${error}。本次检查未完成；如有写入操作请核实记录，不会自动重发。`,true);},
    onResult:result=>{
      $('sync-content').setAttribute('aria-busy','false');$('sync-advanced').hidden=false;
      if(result.simulated) {
        const p=result.preview;
        $('sync-identity').textContent=`示例 · ${p.scope.subjectId} · ${p.scope.tableId}（非真实连接）`;
        status(`${result.label} 共 ${p.items.length} 项；检查时间：${formatContentTime(p.checkedAt)}。`);
        for(const item of p.items) {
          const row=node('section',undefined,'sync-row');row.dataset.state=item.status;
          row.append(node('h3',item.title),node('p',SYNC_LABELS[item.status]??'需要核实','sync-state'),node('p',item.message,'hint'));
          const details=node('details');details.append(node('summary','字段差异与时间（模拟）'));
          for(const d of item.differences)details.append(node('p',`${FIELD_NAMES[d.field]??d.field}：本地 ${formatDifference(d.field,d.local)} ／ 飞书 ${formatDifference(d.field,d.cloud)}`));
          details.append(node('p',`本地修改：${formatContentTime(item.localEvidence.modifiedAt)}；飞书修改：${formatContentTime(item.cloudEvidence?.modifiedAt)}；上次成功同步：${formatContentTime(item.lastSuccessfulSyncAt)}。以上均为示例。`,'hint'));
          row.append(details);$('sync-content').append(row);
        }
      } else if(result.kind==='pairingPreview') {
        const canLink=canSavePairing()||(!isDemo()&&canLinkPairing()&&Number.isSafeInteger(result.contextVersion)&&result.contextVersion>=0);
        const selectionGeneration=viewGeneration,pairingTarget=cloudSettings?.targetKey();
        const formalBatch=!isDemo()&&canBatch()&&Number.isSafeInteger(result.contextVersion)&&result.contextVersion>=0&&cloudSettings?.canInspect();
        const batchContext={generation:selectionGeneration,targetKey:pairingTarget,contextVersion:result.contextVersion};
        $('sync-identity').textContent=`${inspectionLabel} · ${result.accountName} · 账号标识 ${result.subjectId} · 应用 ${result.appId} · 表格 ${result.appToken} / ${result.tableId}`;
        status(savedNotice||`当前目标：${result.accountName} · 数据表 ${result.tableId}。已读取 ${result.cloudCandidates.length} 项，其中 ${result.existingLinks.length} 项已关联本地。`);savedNotice='';
        const quick=$('sync-primary');quick.replaceChildren();
        const linkedCloudIds=new Set(result.existingLinks.map(link=>link.cloudId));
        const cloudOnly=result.cloudCandidates.filter(record=>!linkedCloudIds.has(record.recordId));
        quick.append(node('h3',cloudOnly.length?`可导入本地 · ${cloudOnly.length} 项`:'飞书任务已读取'));
        if(!result.cloudCandidates.length)quick.append(node('p','这张飞书表还没有任务。','hint'));
        else if(!cloudOnly.length)quick.append(node('p','没有尚未关联的飞书任务。已关联任务可在下方“高级操作”中对照更新。','hint'));
        const hiddenCloudCards=[],batchSelections=[];
        for(const [index,record] of cloudOnly.entries()){
          const card=node('article',undefined,'sync-quick-task');
          if(index>=5){card.hidden=true;hiddenCloudCards.push(card);}
          card.append(node('h4',record.title||'未命名任务'),node('p',`${record.progress}% · ${record.status||'状态未知'} · 计划 ${record.plannedDate||'未设置'}`,'hint'));
          const fields=node('details');fields.append(node('summary','查看将导入的 7 项内容'));
          for(const [field,label] of Object.entries(FIELD_NAMES))fields.append(node('p',`${label}：${Object.hasOwn(record.sharedFields??{},field)?formatDifference(field,record.sharedFields[field]):'未返回，不能导入'}`));
          card.append(fields);
          const pending=(result.uploadOperations??[]).some(op=>op.cloudId===record.recordId&&['pending','conflict'].includes(op.state));
          const ready=!isDemo()&&canDownload()&&cloudSettings?.canInspect()===true&&Number.isSafeInteger(result.contextVersion)
            &&result.contextVersion>=0&&Number.isSafeInteger(record.modifiedAt)&&record.modifiedAt>0
            &&record.sharedFields&&typeof record.sharedFields==='object'
            &&Object.keys(FIELD_NAMES).every(field=>Object.hasOwn(record.sharedFields,field))&&!pending;
          if(ready){
            const choice=node('label',undefined,'check'),check=node('input');check.type='checkbox';
            choice.append(check,node('span','选择这条任务导入本地'));
            batchSelections.push({record,check});card.append(choice);
          }else {
            const missing=Object.keys(FIELD_NAMES).filter(field=>!Object.hasOwn(record.sharedFields??{},field));
            const reason=pending?'这条任务有待核实的写入操作，暂不能导入。'
              :missing.length?`飞书表缺少可用于同步的字段：${missing.map(field=>FIELD_NAMES[field]).join('、')}。请在浮窗设置中“检查任务空间”，核对后确认补齐；现有任务不会因此修改。`
              :!Number.isSafeInteger(record.modifiedAt)||record.modifiedAt<=0?'飞书未返回可核实的修改时间，暂不能导入。'
              :!canDownload()||!cloudSettings?.canInspect()?'本地工作台尚未启用当前飞书连接的只读访问，暂不能导入。'
              :'当前连接或记录版本尚未核实完整，请重新读取。';
            card.append(node('p',reason,'hint'));
          }
          quick.append(card);
        }
        if(hiddenCloudCards.length){const showMore=node('button',`查看其余 ${hiddenCloudCards.length} 项`);showMore.type='button';showMore.onclick=()=>{for(const card of hiddenCloudCards)card.hidden=false;showMore.hidden=true;};quick.append(showMore);}
        if(batchSelections.length){
          const actions=node('section',undefined,'sync-import-actions'),summary=node('p','尚未选择任务。','hint');
          const consent=node('label',undefined,'check'),approved=node('input'),importSelected=node('button','导入所选任务','primary');
          approved.type='checkbox';approved.disabled=true;importSelected.type='button';importSelected.disabled=true;
          consent.append(approved,node('span','我已核对所选任务及当前飞书目标，确认新增本地副本并建立关联'));
          const chosen=()=>batchSelections.filter(item=>item.check.checked).map(item=>item.record);
          const update=()=>{const selected=chosen();summary.textContent=selected.length?`已选择 ${selected.length} 项：${selected.map(item=>item.title||'未命名任务').join('、')}`:'尚未选择任务。';
            approved.checked=false;approved.disabled=!selected.length;importSelected.disabled=true;importSelected.textContent=`导入所选 ${selected.length} 项到本地`;};
          for(const item of batchSelections)item.check.onchange=update;
          approved.onchange=()=>{importSelected.disabled=!approved.checked||!chosen().length;};
          importSelected.onclick=async()=>{
            const selected=chosen();
            if(syncing||!approved.checked||!selected.length||selectionGeneration!==viewGeneration||!cloudSettings?.canInspect())return;
            syncing=true;importSelected.disabled=true;approved.disabled=true;for(const item of batchSelections)item.check.disabled=true;
            $('sync-close').disabled=true;$('sync-cloud-check').disabled=true;$('sync-example').disabled=true;
            status(`正在重新核对所选 ${selected.length} 条飞书任务；全部核实后才写入本地…`);
            try{
              const updated=await invoke('import_local_feishu_tasks',{targetKey:pairingTarget,input:{contextVersion:result.contextVersion,confirmImport:true,
                imports:selected.map(record=>({cloudId:record.recordId,subjectId:result.subjectId,expectedCloudModifiedAt:record.modifiedAt,
                  expectedCloudFields:record.sharedFields,confirmImport:true}))}});
              if(!Array.isArray(updated?.tasks))throw Error('本地导入返回结果不完整，请刷新任务核对');
              onSynced(updated);
              if(selectionGeneration===viewGeneration){viewGeneration++;loader.cancel();quick.replaceChildren();$('sync-content').replaceChildren();$('sync-advanced').hidden=true;$('sync-identity').textContent='';status(`已将 ${selected.length} 条任务导入本地并建立关联。飞书未修改；请手动重新读取查看新对照。`);}
            }catch(error){if(selectionGeneration===viewGeneration){viewGeneration++;loader.cancel();quick.replaceChildren();$('sync-content').replaceChildren();$('sync-advanced').hidden=true;$('sync-identity').textContent='';status(`${error}。整批未确认导入；请刷新本地任务核对，再手动重新读取。`,true);}}
            finally{syncing=false;$('sync-close').disabled=false;$('sync-cloud-check').disabled=!canInspectCloud()||!cloudSettings?.canInspect();$('sync-example').disabled=false;}
          };
          actions.append(summary,consent,importSelected);quick.append(actions);
        }
        if(result.existingLinks.length){
          const linked=node('section',undefined,'sync-linked-summary');
          linked.append(node('h3',`已关联本地 · ${result.existingLinks.length} 项`),node('p','可查看两端差异，并选择“保留飞书内容 → 更新本地”。更新前仍需逐条确认。','hint'));
          const open=node('button','查看已关联任务的更新');open.type='button';
          open.onclick=()=>{$('sync-advanced').open=true;$('sync-advanced').scrollIntoView?.({block:'start'});};
          linked.append(open);quick.append(linked);
        }
        for(const warning of result.warnings)$('sync-content').append(node('p',warning,'hint'));
        $('sync-content').append(renderBatchPreview({result,node,formatDifference,fieldNames:FIELD_NAMES,allowUpload:allowUpload()&&(!formalBatch||cloudSettings?.canUpload()===true),scopeLabel:formalBatch?'当前飞书连接':'仅隔离表',execute:formalBatch?plan=>runBatch(()=>invoke('prepare_local_feishu_batch',{targetKey:pairingTarget,input:{contextVersion:result.contextVersion,confirmBatch:true,batch:batchInput(plan)}}),batchContext):canSavePairing()?plan=>runBatch(()=>invoke('local_prepare_isolated_feishu_batch',{input:batchInput(plan)})):undefined}));
        for(const job of result.batchJobs??[]) {
          const section=node('section',undefined,'sync-row');section.append(node('h3',`保存的批次 · ${job.completed}/${job.total}`),node('p',job.message,'hint'));
          const details=node('details');details.append(node('summary','核对保存的方向、内容与版本'));
          for(const e of job.entries??[]) {
            details.append(node('p',`${e.input.localId} ↔ ${e.input.cloudId} · ${e.direction==='local'?'本地 → 飞书':'飞书 → 本地'} · ${e.state} · 本地版本 ${e.input.expectedLocalRevision}`));
            for(const key of Object.keys(FIELD_NAMES))details.append(node('p',`${FIELD_NAMES[key]}：本地 ${formatDifference(key,e.localContent?.[key])} ／ 飞书 ${formatDifference(key,e.input.expectedCloudFields?.[key])}`));
          }
          section.append(details);
          if((canSavePairing()||formalBatch)&&job.state==='uncertain') {
            const verify=node('button','只读核实此批次（不重发、不自动继续）');verify.type='button';
            verify.onclick=async()=>{
              if(syncing||selectionGeneration!==viewGeneration)return;syncing=true;verify.disabled=true;$('sync-close').disabled=true;$('sync-cloud-check').disabled=true;$('sync-example').disabled=true;
              try{const checked=await invoke(formalBatch?'reconcile_local_feishu_batch':'local_reconcile_isolated_feishu_batch',formalBatch?{targetKey:pairingTarget,contextVersion:result.contextVersion,batchId:job.id}:{batchId:job.id});savedNotice=`批次 ${checked.completed}/${checked.total}：${checked.message} 核实没有发送写入，未开始条目仍需人工确认。`;}
              catch(error){savedNotice=`${error}；本次只读核实，没有重发，也不会自动继续。`;}
              try{if(formalBatch){if(selectionGeneration===viewGeneration){viewGeneration++;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-identity').textContent='';status(`${savedNotice} 请手动重新检查。`);savedNotice='';}}else await loader.load('local_inspect_isolated_feishu_pairing',validatePairing);}
              finally{syncing=false;$('sync-close').disabled=false;$('sync-cloud-check').disabled=!canInspectCloud()||(!isDemo()&&!cloudSettings?.canInspect());$('sync-example').disabled=false;}
            };section.append(verify);
          }
          if((canSavePairing()||formalBatch)&&['ready','stopped'].includes(job.state)) {
            const check=node('input');check.type='checkbox';const label=node('label',undefined,'check'),resume=node('button','人工启动／继续该批次');resume.disabled=true;
            label.append(check,node('span','我已核对保存的方向和覆盖内容，确认继续；版本变化时停止，不自动重发'));check.onchange=()=>{resume.disabled=!check.checked;};
            resume.onclick=()=>{if(check.checked&&!syncing&&selectionGeneration===viewGeneration)runBatch(()=>formalBatch?invoke('start_local_feishu_batch',{targetKey:pairingTarget,contextVersion:result.contextVersion,batchId:job.id,confirmContinue:true}):job.state==='stopped'?invoke('local_control_isolated_feishu_batch',{batchId:job.id,action:'resume'}):Promise.resolve(job),formalBatch?batchContext:undefined);};section.append(label,resume);
          }
          $('sync-content').append(section);
        }
        for(const comparison of result.comparisons??[]) {
          const section=node('section',undefined,'sync-row');
          section.dataset.state=comparison.status??'manual';
          section.append(node('h3',`${comparison.localTitle} ↔ ${comparison.cloudTitle}`));
          section.append(node('p',SYNC_LABELS[comparison.status]??'需要人工判断','sync-state'),node('p',comparison.message??'无法可靠判断更新方向，请核实。','hint'));
          const conflict=node('p',conflictWarning(comparison),'sync-conflict-warning');
          conflict.hidden=comparison.status!=='conflict';
          if(!conflict.hidden)conflict.setAttribute('role','alert');
          section.append(conflict);
          section.append(node('p',`本地修改：${formatContentTime(comparison.localModifiedAt)}；飞书修改：${formatContentTime(comparison.cloudModifiedAt)}`,'hint'));
          const table=node('table',undefined,'sync-comparison'),head=node('tr');
          for(const title of ['字段','本地','飞书','比较结果'])head.append(node('th',title));
          const thead=node('thead');thead.append(head);table.append(thead);
          const body=node('tbody');
          for(const field of comparison.fields) {
            const row=node('tr');
            for(const text of [FIELD_NAMES[field.field]??field.field,formatDifference(field.field,field.local),field.available?formatDifference(field.field,field.cloud):'无法核实（未返回字段）',!field.available?'待核实':field.equal?'一致':'不同'])row.append(node('td',text));
            body.append(row);
          }
          table.append(body);section.append(table);
          section.append(node('p',comparison.baselineEstablished?`已有同步基线；上次成功同步：${formatContentTime(comparison.lastSuccessfulSyncAt)}。当前${comparison.baselineCandidateReady?'两侧一致。':'两侧有差异，请核实后选择。'}`:comparison.baselineCandidateReady?'共有内容全部一致，可作为首次基线候选；当前尚未保存基线。':'首次基线尚未就绪：请核实缺失字段，并选择需要保留的一侧内容。','hint'));
          const label=node('label','同步方向（确认前预览）'),select=node('select');
          select.setAttribute('aria-label','同步方向（确认前预览）');
          for(const [value,text] of [['','请选择'],['local','保留本地内容 → 更新飞书'],['cloud','保留飞书内容 → 更新本地']]) {if(value==='local'&&(!allowUpload()||(!isDemo()&&!canUpload())))continue;const option=node('option',text);option.value=value;select.append(option);}
          const notice=node('p','选择方向后会显示变更预览，本页只做比较。','hint');
          const confirm=node('button',comparison.baselineCandidateReady?'确认保存一致基线':'确认从飞书更新此本地任务（可撤销）','primary');confirm.type='button';confirm.hidden=true;
          const consent=node('label',undefined,'check'),checkbox=node('input');checkbox.type='checkbox';
          const consentText=node('span');consent.append(checkbox,consentText);consent.hidden=true;
          const formalDownload=()=>!isDemo()&&canDownload()&&Number.isSafeInteger(result.contextVersion)&&result.contextVersion>=0
            &&comparison.subjectId===result.subjectId&&cloudSettings?.canInspect()===true;
          const formalUpload=()=>!isDemo()&&canUpload()&&Number.isSafeInteger(result.contextVersion)&&result.contextVersion>=0
            &&comparison.subjectId===result.subjectId&&cloudSettings?.canUpload()===true;
          const hasPending=()=>(result.uploadOperations??[]).some(op=>(op.localId===comparison.localId||op.cloudId===comparison.cloudId)&&['pending','conflict'].includes(op.state));
          confirm.disabled=true;checkbox.onchange=()=>{confirm.disabled=!checkbox.checked;};
          select.onchange=()=>{
            checkbox.checked=false;confirm.disabled=true;
            conflict.textContent=conflictWarning(comparison,select.value);
            const complete=comparison.fields.every(field=>field.available);
            const pending=hasPending();
            confirm.hidden=consent.hidden=!(select.value&&complete&&(canSavePairing()||(select.value==='cloud'&&formalDownload())||(select.value==='local'&&formalUpload()))&&!pending);
            confirm.textContent=comparison.baselineCandidateReady?'确认保存一致基线':select.value==='local'?'确认更新此飞书记录':'确认从飞书更新此本地任务（可撤销）';
            consentText.textContent=comparison.baselineCandidateReady?'我已核对两侧内容一致，确认保存同步基线':select.value==='local'?'我已核对上述字段，确认用本地内容覆盖此飞书记录的共有字段':'我已核对上述字段，确认用飞书内容更新这条本地任务';
            if(comparison.status==='conflict')consentText.textContent+=`；我理解这会舍弃${select.value==='local'?'飞书':'本地'}相对基线的修改，不会自动合并`;
            if(!select.value){notice.textContent='选择方向后会显示变更预览，本页只做比较。';return;}
            const changed=comparison.fields.filter(field=>field.available&&!field.equal);
            const unknown=comparison.fields.filter(field=>!field.available);
            notice.textContent=`方向预览：${select.value==='local'?'本地 → 飞书':'飞书 → 本地'}；${changed.length} 个已核实字段不同${changed.length?'（'+changed.map(field=>FIELD_NAMES[field.field]).join('、')+'）':''}。${unknown.length?`${unknown.length} 个字段待核实，执行前需要补齐。`:''}${pending?'此前写入待核实，请先处理下方操作记录。':select.value==='local'?'单次提交共有字段与相应状态；未映射字段不写入。':'勾选并点击确认后更新本地，保留撤销记录。'}`;
          };
          confirm.onclick=async()=>{
            if(syncing||!checkbox.checked||!select.value)return;
            if(selectionGeneration!==viewGeneration||hasPending()||!comparison.fields.every(field=>field.available))return;
            if(!canSavePairing()&&!(select.value==='cloud'&&formalDownload())&&!(select.value==='local'&&formalUpload()))return;
            const input={localId:comparison.localId,cloudId:comparison.cloudId,subjectId:comparison.subjectId,expectedLocalRevision:comparison.localRevision,expectedLocalModifiedAt:comparison.localModifiedAt,expectedCloudModifiedAt:comparison.cloudModifiedAt,expectedCloudFields:comparison.expectedCloudFields};
            if(select.value==='local'&&!comparison.baselineCandidateReady){
              if(isDemo())await uploadAction('local_upload_isolated_feishu_task',{input});
              else await formalUploadAction('upload_local_feishu_task',{targetKey:pairingTarget,input:{contextVersion:result.contextVersion,confirmOverwrite:true,upload:input}},selectionGeneration);
              return;
            }
            confirm.disabled=true;select.disabled=true;checkbox.disabled=true;
            syncing=true;$('sync-close').disabled=true;$('sync-cloud-check').disabled=true;$('sync-example').disabled=true;
            status('正在复查两侧版本并更新本地任务…');
            try {
              const updated=await invoke(isDemo()?'local_download_isolated_feishu_task':'download_local_feishu_task',isDemo()?{input}:{targetKey:pairingTarget,input:{contextVersion:result.contextVersion,download:input}});
              if(!isDemo()&&!Array.isArray(updated?.tasks))throw new Error('本地下载返回结果不完整，请刷新任务核对');
              onSynced(updated);
              if(selectionGeneration!==viewGeneration)return;
              const notice=comparison.baselineCandidateReady?'两侧内容一致；已保存同步基线，任务内容未改变。':'飞书内容已更新到本地，读回一致；已保存同步基线和撤销记录。关闭面板后可撤销。';
              if(isDemo()){savedNotice=notice;await loader.load('local_inspect_isolated_feishu_pairing',validatePairing);}
              else {viewGeneration++;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-identity').textContent='';status(`${notice} 请手动重新检查查看同步结果；没有向飞书写入。`);}
            } catch(error) {
              if(selectionGeneration!==viewGeneration)return;
              if(isDemo()){status(`${error}。请重新检查后确认；不会自动重试。`,true);select.disabled=false;checkbox.disabled=false;confirm.disabled=false;}
              else {viewGeneration++;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-identity').textContent='';status(`${error}。请先刷新本地任务核对结果，再重新检查；不会自动重试。`,true);}
            }
            finally {syncing=false;$('sync-close').disabled=false;$('sync-cloud-check').disabled=!canInspectCloud()||(!isDemo()&&!cloudSettings?.canInspect());$('sync-example').disabled=false;}
          };
          label.append(select);section.append(label,notice,consent,confirm);$('sync-content').append(section);
        }
        if(result.uploadOperations?.length) {
          const operations=node('section',undefined,'sync-row');operations.append(node('h3','飞书写入操作记录'));
          for(const operation of result.uploadOperations) {
            const row=node('section',undefined,'sync-row');row.append(node('p',`${operation.cloudId} · ${({pending:'待核实',completed:'已核实完成',notApplied:'写入未发送',conflict:'内容已变化，需核对',restored:'已恢复原内容'})[operation.state]??operation.state}`));
            if(operation.isRestore===true)row.append(node('p','这是恢复上传前内容的操作；不改变本地任务或同步基线。','hint'));
            if((canSavePairing()||(!isDemo()&&canReconcile()&&Number.isSafeInteger(result.contextVersion)&&cloudSettings?.canInspect()))&&['pending','conflict'].includes(operation.state)){
              const verify=node('button','只读核实写入结果');
              verify.onclick=()=>isDemo()?uploadAction('local_reconcile_isolated_feishu_upload',{operationId:operation.id})
                :formalUploadAction('reconcile_local_feishu_upload',{targetKey:pairingTarget,contextVersion:result.contextVersion,operationId:operation.id},selectionGeneration);
              row.append(verify);
            }
            if(operation.canRestore) {
              const preview=node('details');preview.append(node('summary','查看上传前的恢复内容'));
              for(const [field,value] of Object.entries(operation.restoreContent))preview.append(node('p',`${FIELD_NAMES[field]??field}：${formatDifference(field,value)}`));
              if(typeof operation.restoreStatus==='string')preview.append(node('p',`状态：${operation.restoreStatus||'未设置'}`));
              row.append(preview);
              const pending=(result.uploadOperations??[]).some(op=>(op.cloudId===operation.cloudId||op.localId===operation.localId)&&['pending','conflict'].includes(op.state));
              const formalRestore=!isDemo()&&canRestore()&&Number.isSafeInteger(result.contextVersion)&&result.contextVersion>=0
                &&typeof operation.restoreStatus==='string'&&cloudSettings?.canUpload()===true;
              if(pending)row.append(node('p','该记录仍有待核实操作，请先只读核实，不能再次恢复。','hint'));
              if(!pending&&(canSavePairing()||formalRestore)) {
              if(formalRestore)preview.open=true;
              const consent=node('label',undefined,'check'),check=node('input'),restore=node('button','恢复上传前的飞书内容');check.type='checkbox';restore.disabled=true;
              consent.append(check,node('span','已核对恢复内容；仅在飞书仍是该次上传版本时恢复，不修改本地任务'));
              check.onchange=()=>{restore.disabled=!check.checked;};
              restore.onclick=()=>{
                if(!check.checked||syncing)return;
                if(isDemo())return uploadAction('local_restore_isolated_feishu_upload',{operationId:operation.id});
                return formalUploadAction('restore_local_feishu_upload',{targetKey:pairingTarget,input:{contextVersion:result.contextVersion,operationId:operation.id,
                  confirmRestore:true,expectedRestoreContent:operation.restoreContent,expectedRestoreStatus:operation.restoreStatus}},selectionGeneration);
              };
              row.append(consent,restore);
              }
            }
            operations.append(row);
          }
          $('sync-content').append(operations);
        }
        if(result.existingLinks.length) {
          const linked=node('section',undefined,'sync-row');linked.dataset.state='equal';linked.append(node('h3','已保存的对应关系'));
          for(const link of result.existingLinks) {
            const local=result.localCandidates.find(record=>record.id===link.localId);
            const cloud=result.cloudCandidates.find(record=>record.recordId===link.cloudId);
            linked.append(node('p',`${local?.title||link.localId} ↔ ${cloud?.title||link.cloudId}`));
          }
          linked.append(node('p',result.existingLinks.some(link=>link.baselineEstablished)?'已建立同步基线；具体时间和字段见上方对照。':'仅保存 ID 对应；尚未建立同步基线。','hint'));$('sync-content').append(linked);
        }
        const linkedLocal=new Set(result.existingLinks.map(link=>link.localId));
        const linkedCloud=new Set(result.existingLinks.map(link=>link.cloudId));
        const cloud=node('section',undefined,'sync-row');cloud.dataset.state='cloudUnlinked';cloud.append(node('h3','飞书候选'));
        for(const record of result.cloudCandidates) {
          const text=`${record.title||'未命名记录'} · ${record.progress}% · ${record.status||'状态未知'} · 计划 ${record.plannedDate||'未设置'} · 修改 ${formatContentTime(record.modifiedAt)} · ${record.recordId}`;
          if(canLink&&!linkedCloud.has(record.recordId)) {const label=node('label',undefined,'check');const radio=node('input');radio.type='radio';radio.name='pairing-cloud';radio.value=record.recordId;label.append(radio,node('span',text));cloud.append(label);} else cloud.append(node('p',text));
        }
        $('sync-content').append(cloud);
        const local=node('section',undefined,'sync-row');local.dataset.state='unlinked';local.append(node('h3','本地候选'));
        for(const record of result.localCandidates) {
          const text=`${record.title} · ${record.progress}% · 计划 ${record.plannedDate||'未设置'} · 修改 ${formatContentTime(record.modifiedAt)} · ${record.id}`;
          if(canLink&&!linkedLocal.has(record.id)) {const label=node('label',undefined,'check');const radio=node('input');radio.type='radio';radio.name='pairing-local';radio.value=record.id;label.append(radio,node('span',text));local.append(label);} else local.append(node('p',text));
        }
        $('sync-content').append(local);
        if(canLink&&result.localCandidates.some(record=>!linkedLocal.has(record.id))&&result.cloudCandidates.some(record=>!linkedCloud.has(record.recordId))) {
          const actions=node('div',undefined,'dialog-actions'),save=node('button','保存所选对应关系','primary');save.type='button';save.disabled=true;
          const selected=()=>({localId:panel.querySelector('input[name="pairing-local"]:checked')?.value,cloudId:panel.querySelector('input[name="pairing-cloud"]:checked')?.value});
          panel.querySelectorAll('input[name="pairing-local"],input[name="pairing-cloud"]').forEach(input=>input.onchange=()=>{const choice=selected();save.disabled=!(choice.localId&&choice.cloudId);});
          save.onclick=async()=>{
            if(syncing||selectionGeneration!==viewGeneration||(!isDemo()&&!cloudSettings?.canInspect()))return;
            const choice=selected(),localRecord=result.localCandidates.find(record=>record.id===choice.localId),cloudRecord=result.cloudCandidates.find(record=>record.recordId===choice.cloudId);
            if(!localRecord||!cloudRecord)return;
            syncing=true;save.disabled=true;$('sync-close').disabled=true;$('sync-cloud-check').disabled=true;$('sync-example').disabled=true;
            status('正在保存对应关系；保存前会重新读取两端版本，不修改任务内容…');
            const pairing={localId:localRecord.id,cloudId:cloudRecord.recordId,expectedLocalRevision:localRecord.revision,expectedLocalModifiedAt:localRecord.modifiedAt,expectedCloudModifiedAt:cloudRecord.modifiedAt};
            try{
              const saved=await invoke(isDemo()?'local_save_isolated_feishu_pairing':'save_local_feishu_pairing',isDemo()?{input:pairing}:{targetKey:pairingTarget,input:{contextVersion:result.contextVersion,subjectId:result.subjectId,pairing}});
              if(selectionGeneration!==viewGeneration)return;
              if(saved?.kind!=='pairingSaved'||saved.baselineEstablished!==false||saved.successfulSyncRecorded!==false||saved.localTaskChanged!==false||saved.cloudRecordChanged!==false)throw new Error('保存结果不完整，未确认对应关系');
              const notice=saved.alreadySaved?'该对应关系已经保存；未修改任务或基线。':'对应关系已保存；未修改任务内容，尚未建立同步基线。';
              if(isDemo()){savedNotice=notice;await loader.load('local_inspect_isolated_feishu_pairing',validatePairing);}
              else {viewGeneration++;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-identity').textContent='';status(`${notice} 请手动重新检查查看关联结果。`);}
            }catch(error){if(selectionGeneration===viewGeneration){status(`${error}。没有写入任务内容，请重新检查。`,true);save.disabled=false;}}
            finally{syncing=false;$('sync-close').disabled=false;$('sync-example').disabled=false;$('sync-cloud-check').disabled=!canInspectCloud()||(!isDemo()&&!cloudSettings?.canInspect());}
          };
          actions.append(save);$('sync-content').append(actions);
        }
      } else {
        $('sync-identity').textContent=`${inspectionLabel} · ${result.accountName} · 账号标识 ${result.subjectId} · 应用 ${result.appId} · 表格 ${result.appToken} / ${result.tableId}`;
        status(`读取完整：${result.records.length} 条记录。这是连接与时间检查，尚未建立本地对应关系或同步基线。`);
        for(const warning of result.warnings)$('sync-content').append(node('p',warning,'hint'));
        for(const record of result.records) {
          const row=node('section',undefined,'sync-row');row.append(node('h3',record.title||'未命名记录'),node('p',`飞书修改：${formatContentTime(record.modifiedAt)}；记录 ${record.recordId}`,'hint'));$('sync-content').append(row);
        }
      }
    },
  });
  let cloudSettings;
  function formalSettings() {
    if(isDemo())return null;
    if(!cloudSettings){const host=node('section',undefined,'sync-access-card');$('sync-access').append(host);
      cloudSettings=initCloudSettings({container:host,invoke,
        onPermission:enabled=>{$('sync-cloud-check').disabled=!enabled||!canInspectCloud();$('sync-cloud-check').textContent=enabled?'读取飞书任务（只读）':'先开启云端访问';},
        onInvalidate:()=>{viewGeneration++;loader.cancel();savedNotice='';$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-advanced').hidden=true;$('sync-identity').textContent='';status('连接设置已变化；请手动重新读取飞书任务。');}});}
    return cloudSettings;
  }
  function invalidateConnection(message='账号或目标连接可能已变化；旧预览已清除。请关闭后重新打开同步面板，再手动检查；不会自动同步。') {
    viewGeneration++;loader.cancel();savedNotice='';cloudSettings?.cancel();
    activeBatchStop?.();
    $('sync-cloud-check').disabled=true;$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-advanced').hidden=true;
    $('sync-content').setAttribute('aria-busy','false');$('sync-identity').textContent='';status(message);
  }
  if(listen)Promise.resolve(listen('feishu-connection-changed',()=>{if(!isDemo())invalidateConnection();}))
    .catch(()=>invalidateConnection('连接变更通知不可用；旧预览已清除，请重新打开面板核对连接。'));
  function close(){if(syncing)return;viewGeneration++;loader.cancel();cloudSettings?.cancel();panel.close();$('sync-button').focus();}
  $('sync-button').onclick=()=>{
    if(syncing||!canOpen())return;
    auth.refresh();
    $('sync-cloud-check').disabled=!canInspectCloud();
    $('sync-cloud-check').textContent=isDemo()&&canInspectCloud()?'检查隔离测试表（只读）':'读取飞书任务（只读）';
    $('sync-cloud-hint').textContent=canInspectCloud()
      ? (isDemo()?'仅用于隔离验收；读取不会写入。':'本次仅读取。导入、更新或上传均需单独确认。')
      :'当前构建没有云端读取能力。';
    panel.showModal();$('sync-example').hidden=!isDemo();
    if(isDemo()){$('sync-example').focus();loader.load('local_sync_demo',assertDemo);}
    else {viewGeneration++;loader.cancel();$('sync-content').replaceChildren();$('sync-primary').replaceChildren();$('sync-advanced').hidden=true;$('sync-advanced').open=false;$('sync-identity').textContent='';status('正在检查本地连接设置；随后点击“读取飞书任务”。');$('sync-close').focus();}
    const settings=formalSettings();if(settings){$('sync-cloud-check').disabled=true;settings.refresh().then(()=>{
      if(!settings.canInspect()){$('sync-connection-details').open=true;status('请在“连接与权限设置”中开启云端访问，再读取飞书任务。');}
    });}
  };
  $('sync-example').onclick=()=>{if(!syncing&&isDemo())loader.load('local_sync_demo',assertDemo);};
  $('sync-cloud-check').onclick=()=>{
    if(syncing)return;
    if(!canInspectCloud())return;
    if(!isDemo()&&!cloudSettings?.canInspect())return;
    if(auth.isActive())return;
    const command=isDemo()?'local_inspect_isolated_feishu_pairing':'inspect_local_feishu_sync';
    loader.load(command,validatePairing);
  };
  function validatePairing(result) {
      const pairing=result?.kind==='pairingPreview'&&Array.isArray(result.localCandidates)&&Array.isArray(result.cloudCandidates)&&result.requiresExplicitSelection===true;
      const inspection=Array.isArray(result?.records)&&result?.complete===true;
      if(result?.readOnly!==true||(!pairing&&!inspection))throw new Error('飞书读取不完整，停止检查');return result;
  }
  $('sync-close').onclick=close;
  panel.addEventListener('cancel',e=>{e.preventDefault();close();});
  return {invalidateConnection};
}
