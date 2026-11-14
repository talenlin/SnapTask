import {createContextCoach} from './context-coach.mjs';
export const GUIDE_KEY='project-track.workbench-coach.v2';
const main=(target,title,text,extra={})=>({target,title,text,...extra});
const field=(target,title,text,next)=>({resolve:doc=>doc.querySelector('#editor').open?{target:doc.querySelector(target)}:{target:doc.querySelector('#add'),title:'可以继续录入，也可以先看看其他功能',text:'点击“新增任务”回到表单；或选“下一处”跳过录入，不会自动创建任务。',next:'board'},title,text,next,clickTarget:'#add',clickNext:'title',clickWhen:doc=>doc.querySelector('#editor').open});
const syncField=(target,title,text)=>({resolve:doc=>doc.querySelector('#sync-panel').open?{target:doc.querySelector(target)}:{target:doc.querySelector('#sync-button'),title:'先打开同步面板',text:'点击此按钮继续查看实际连接控件；也可以跳过。只打开面板不会访问云端。',next:'backup-entry'},title,text,clickTarget:'#sync-button',clickNext:'connection',clickWhen:doc=>doc.querySelector('#sync-panel').open});
export const GUIDE_TRACKS={
  local:[
    {id:'add',...main('#add','从一条本地任务开始','点击这里打开表单。无需登录飞书或 WPS；引导不会替你创建示例任务。',{clickTarget:'#add',clickNext:'title',clickWhen:doc=>doc.querySelector('#editor').open,next:'board',nextLabel:'跳过录入'})},
    {id:'title',...field('#editor [name="title"]','填写你自己的任务名称','例如“整理本周工作计划”。先填写名称，再选“下一处”了解进度；不会自动填入示例。','progress')},
    {id:'progress',...field('#editor [name="progress"]','这是完成后的总进度','填写 0–100 的整数。0% 未开始，1–99% 进行中，100% 已完成；回退进度时，本地状态也会回退。','save')},
    {id:'save',...field('#save','由你决定是否保存','四象限和日期可按需填写。点击真实的“保存任务”才写入本地；成功后提示会跟到看板入口。','board'),event:'task-saved',eventNext:'board',nextLabel:'跳过此项'},
    {id:'board',...main('[data-view="board"]','按状态观察任务','点击状态看板查看分组；跨状态拖动时需要确认进度。这里只提示，不会替你拖动或更改任务。')},
    {id:'quadrants',...main('[data-view="quadrants"]','用四象限分清轻重缓急','点击此处查看四象限。拖到其他象限只改变重要、紧急分类；误操作可以撤销。')},
    {id:'calendar',...main('[data-view="calendar"]','按计划日期安排工作','计划日历按“计划完成日期”显示任务；没填日期的任务在未排期区域，不会丢失。')},
    {id:'stats',...main('[data-view="stats"]','看完成率，也看平均进度','完成率是已完成任务占比；平均进度是当前可见任务的进度均值。两者不是同一个指标。')},
    {id:'search',...main('#scope','只看今天或需要跟进的任务','这里可筛选今日待办、逾期未完成、已完成和回收站。左边搜索框可以搜名称与详情，筛选也会影响统计。')},
    {id:'undo',...main('#undo','改错了可以撤销','默认 Ctrl+Z 撤销任务操作；输入文字时优先撤销输入。可在“设置 · 快捷键”修改按键。',{nextLabel:'完成本地引导'})},
  ],
  sync:[
    {id:'sync-entry',...main('#sync-button','需要云端时再打开这里','飞书是可选项。点击打开同步面板，只读取本地保存的连接状态，不会自动同步。',{clickTarget:'#sync-button',clickNext:'connection',clickWhen:doc=>doc.querySelector('#sync-panel').open,next:'backup-entry',nextLabel:'稍后连接'})},
    {id:'connection',...syncField('[data-coach="cloud-setup"]','先初始化或核对飞书连接','点击真实按钮进入主 App 的飞书连接流程。请用飞书身份而非豆包账号；在该页面点“连接步骤提示”可继续跟随控件。')},
    {id:'access',...syncField('[data-coach="cloud-access"]','云端访问由你手动开启','先核对连接的目标表。按钮不可用时先配置连接；开启开关本身不会读任务或执行上传。')},
    {id:'check',...syncField('#sync-cloud-check','手动检查实际账号和差异','由你点击检查，才只读获取飞书任务。连接配置、浏览器账号与实际授权身份可能不同，请以检查结果为准。')},
    {id:'compare',...syncField('#sync-status','关联后，再选择保留哪一端','检查成功后在下方选本地/飞书记录并确认关联，再对照字段。下载保留飞书内容；上传保留本地内容。勾选确认后才执行，结果不明时不要重复上传。'),next:'backup-entry'},
    {id:'backup-entry',...main('#backup-button','同步前也可以先备份','关闭同步面板后，从这里导出完整 JSON。备份不含连接和密钥，CSV 不能替代它。',{nextLabel:'完成同步引导'})},
  ],
  backup:[
    {id:'backup-entry',...main('#backup-button','给任务留一条退路','点击打开备份与恢复；仅打开不会导出或替换任务。',{clickTarget:'#backup-button',clickNext:'export',clickWhen:doc=>doc.querySelector('#backup-panel').open,nextLabel:'跳过备份',next:'undo'})},
    {id:'export',resolve:doc=>doc.querySelector('#backup-panel').open?{target:doc.querySelector('#backup-export')}:{target:doc.querySelector('#backup-button'),next:'undo'},title:'导出完整备份',text:'点击此按钮并由你选择保存位置。完整 JSON 包含任务、回收站与撤销历史，不含连接和密钥。恢复必须先预览，再明确确认。'},
    {id:'undo',...main('#undo','日常操作先试撤销','关闭备份窗口后，可在这里撤销上一步本地任务操作。恢复备份是整体替换，不能当作普通撤销。',{nextLabel:'完成备份引导'})},
  ],
};
export function initWorkbenchGuide({document:doc=document,storage,canOpen=()=>true,onError=()=>{}}) {
  const $=id=>doc.getElementById(id);let initialized=false;
  const tracks=Object.fromEntries(Object.entries(GUIDE_TRACKS).map(([name,steps])=>[name,steps.map(step=>({...step,resolve:d=>{
    const actual=step.resolve?step.resolve(d):{target:d.querySelector(step.target)},dialog=d.querySelector('dialog[open]');
    if(dialog&&actual?.target&&!dialog.contains(actual.target))return {target:dialog.querySelector('#cancel-top,#sync-close,#backup-close,#shortcut-close'),title:'先返回工作台，再继续这一处',text:'请自行保存或取消当前操作，然后关闭面板。引导不会替你放弃输入或中断同步。',blocked:true};
    return actual;
  }}))]));
  const coach=createContextCoach({document:doc,storage,key:GUIDE_KEY,tracks,canStart:()=>initialized&&canOpen(),onError});
  $('guide-open').onclick=()=>coach.menu($('guide-open'),[['local','本地任务 · 跟随操作'],['sync','飞书连接与同步 · 按需查看'],['backup','备份与撤销 · 跟随操作']]);
  doc.addEventListener('local-task-saved',()=>coach.notify('task-saved'));
  return {initialize(){initialized=true;$('guide-open').disabled=false;},active:coach.active};
}
