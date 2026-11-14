// S3 preview and explicitly injected sequential execution. No bridge or auto-retry.
const FIELDS=['title','detail','progress','quadrant','today','startDate','plannedDate'];
const STATUSES=new Set(['localChanged','cloudChanged','conflict','manual','equal','unverified']);
export const pairingKey=row=>JSON.stringify([row.localId,row.cloudId]);
export async function runBatchQueue({start,step,stopped=()=>false,onProgress=()=>{},beforeStep=async()=>{}}) {
  let job=await start();onProgress(job);
  while(!stopped()&&job.state==='ready') {
    await beforeStep(job);
    if(stopped())break;
    const before=job;job=await step(before.id);
    if(job?.id!==before.id||job.total!==before.total||!Number.isSafeInteger(job.completed)||job.completed<before.completed||job.completed>before.completed+1||(job.state==='ready'&&job.completed!==before.completed+1))throw new Error('批次进度不完整，停止调度；请只读核实，不重发');
    onProgress(job);
  }
  return job;
}
export function buildBatchPreview(result,choices=new Map()) {
  if(result?.kind!=='pairingPreview'||result.readOnly!==true||!Array.isArray(result.comparisons))throw new Error('批量预览需要完整的只读检查');
  const comparisons=result.comparisons;
  const localCounts=new Map(),cloudCounts=new Map();
  for(const row of comparisons){localCounts.set(row.localId,(localCounts.get(row.localId)??0)+1);cloudCounts.set(row.cloudId,(cloudCounts.get(row.cloudId)??0)+1);}
  const items=[],blocked=[],equal=[];let unselected=0;
  for(const row of comparisons) {
    let reason='';
    if(!row.localId||!row.cloudId||localCounts.get(row.localId)!==1||cloudCounts.get(row.cloudId)!==1)reason='对应关系重复或无效';
    else if(!STATUSES.has(row.status)||row.status==='unverified')reason='差异分类尚未核实';
    else if(!Array.isArray(row.fields)||row.fields.length!==7||FIELDS.some(key=>row.fields.filter(f=>f.field===key&&f.available===true).length!==1))reason='共有字段缺失或未核实';
    else if((result.uploadOperations??[]).some(op=>(op.localId===row.localId||op.cloudId===row.cloudId)&&['pending','conflict'].includes(op.state)))reason='此前写入结果待核实';
    else if(!Number.isSafeInteger(row.localRevision)||row.localRevision<1||![row.localModifiedAt,row.cloudModifiedAt].every(t=>Number.isSafeInteger(t)&&t>0&&t<=253402300799999))reason='版本或修改时间待核实';
    if(reason){blocked.push({key:pairingKey(row),row,reason});continue;}
    const differences=row.fields.filter(f=>JSON.stringify(f.local)!==JSON.stringify(f.cloud));
    if(!differences.length){equal.push(row);continue;}
    const direction=choices.get(pairingKey(row));
    if(direction!=='local'&&direction!=='cloud'){unselected++;continue;}
    items.push({key:pairingKey(row),row,direction,conflict:row.status==='conflict',differences});
  }
  return {items,blocked,equal,unselected,total:comparisons.length,
    localToCloud:items.filter(x=>x.direction==='local').length,
    cloudToLocal:items.filter(x=>x.direction==='cloud').length,
    conflicts:items.filter(x=>x.conflict).length,
    unmatchedLocal:(result.localCandidates??[]).filter(x=>!(result.existingLinks??[]).some(link=>link.localId===x.id)).length,
    unmatchedCloud:(result.cloudCandidates??[]).filter(x=>!(result.existingLinks??[]).some(link=>link.cloudId===x.recordId)).length,
    missingLinked:(result.existingLinks??[]).filter(link=>!comparisons.some(x=>x.localId===link.localId&&x.cloudId===link.cloudId)).length};
}

export function renderBatchPreview({result,node,formatDifference,fieldNames,execute,allowUpload=true,scopeLabel='仅隔离表'}) {
  const choices=new Map(),root=node('details',undefined,'sync-row');
  root.append(node('summary',execute?`批量同步：预览 → 独立确认执行（${scopeLabel}）`:'批量差异清单与确认预览（不写入）'));
  root.append(node('p',execute?'每条独立选择保留内容；先确认预览，再单独确认实际执行。不自动关联、不自动合并。':'每条独立选择保留内容；不自动关联、不自动合并。这里仅确认预览，不会提交批量同步。','hint'));
  const summary=node('p'),list=node('div'),review=node('div');
  const consent=node('label',undefined,'check'),check=node('input');check.type='checkbox';
  consent.append(check,node('span','我已核对所选方向及覆盖影响；本阶段只确认预览，不执行写入'));
  const confirm=node('button','确认所选预览（不写入）');confirm.type='button';confirm.disabled=true;
  const message=node('p');message.setAttribute('role','status');
  const execution=node('div'),executeConsent=node('label',undefined,'check'),executeCheck=node('input'),run=node('button',`执行所选批量同步（${scopeLabel}）`);
  executeCheck.type='checkbox';run.type='button';run.disabled=true;execution.hidden=true;
  executeConsent.append(executeCheck,node('span','我确认实际执行所选方向，理解会整体覆盖目标一侧的共有字段；冲突修改不会自动合并'));
  execution.append(executeConsent,run);
  executeCheck.onchange=()=>{run.disabled=!executeCheck.checked;};
  const update=()=>{
    check.checked=false;confirm.disabled=true;message.textContent='';review.replaceChildren();
    executeCheck.checked=false;run.disabled=true;execution.hidden=true;
    const plan=buildBatchPreview(result,choices);
    summary.textContent=`已关联对照 ${plan.total} 项；所选 ${plan.items.length} 项（本地 → 飞书 ${plan.localToCloud}，飞书 → 本地 ${plan.cloudToLocal}，双端冲突 ${plan.conflicts}）；未选 ${plan.unselected}，内容一致 ${plan.equal.length}，待核实 ${plan.blocked.length+plan.missingLinked}。未关联本地 ${plan.unmatchedLocal}／飞书 ${plan.unmatchedCloud}，不会新增或删除。`;
    for(const item of plan.items){
      const details=node('details');details.append(node('summary',`${item.row.localTitle} ↔ ${item.row.cloudTitle} · ${item.direction==='local'?'本地 → 飞书':'飞书 → 本地'} · ${item.differences.length} 字段变化`));
      details.append(node('p',`将用${item.direction==='local'?'本地':'飞书'}的共有字段整体覆盖${item.direction==='local'?'飞书':'本地'}；${item.conflict?'双方都有修改，目标一侧相对基线的修改会被舍弃；':''}不会自动合并，未映射字段不在本预览范围。`,'hint'));
      for(const f of item.differences)details.append(node('p',`${fieldNames[f.field]}：${formatDifference(f.field,item.direction==='local'?f.cloud:f.local)} → ${formatDifference(f.field,item.direction==='local'?f.local:f.cloud)}`));
      review.append(details);
    }
    return plan;
  };
  const initial=buildBatchPreview(result);
  for(const row of result.comparisons){
    const key=pairingKey(row),blocked=initial.blocked.find(x=>x.key===key),isEqual=initial.equal.includes(row);
    const label=node('label',`${row.localTitle} ↔ ${row.cloudTitle}`),select=node('select');
    select.setAttribute('aria-label',`批量预览方向：${row.localTitle} ↔ ${row.cloudTitle}`);
    for(const [value,text] of [['','跳过（默认）'],['local','保留本地 → 覆盖飞书'],['cloud','保留飞书 → 覆盖本地']]){if(value==='local'&&!allowUpload)continue;const option=node('option',text);option.value=value;select.append(option);}
    select.disabled=!!blocked||isEqual;select.onchange=()=>{choices.set(key,select.value);update();};
    label.append(select);list.append(label);
    if(blocked||isEqual)list.append(node('p',blocked?.reason??'内容一致，无需覆盖','hint'));
  }
  check.onchange=()=>{confirm.disabled=!check.checked||!buildBatchPreview(result,choices).items.length;};
  confirm.onclick=()=>{if(!check.checked||!buildBatchPreview(result,choices).items.length)return;message.textContent='所选预览已核对；没有提交任何写入，也没有更新同步基线。正式执行前仍需重新读取并复查每条版本。';execution.hidden=!execute;};
  run.onclick=async()=>{if(!execute||!check.checked||!executeCheck.checked||!buildBatchPreview(result,choices).items.length)return;run.disabled=true;try{await execute(buildBatchPreview(result,choices));}catch(error){message.textContent=String(error);executeCheck.checked=false;run.disabled=true;}};
  root.append(summary,list,review,consent,confirm,message,execution);update();return root;
}
