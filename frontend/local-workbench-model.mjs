export const QUADRANTS = { iu: '重要且紧急', in: '重要不紧急', un: '紧急不重要', nn: '不重要不紧急' };
export const STATUSES = ['未开始', '进行中', '已完成'];
export const MOVE_STATUSES = {todo:'未开始', doing:'进行中', done:'已完成'};
export function moveProposal(task, kind, target) {
  if(task.trashed) throw new Error('回收站任务不能移动');
  if(kind==='quadrant' && Object.hasOwn(QUADRANTS,target)) return {changed:task.quadrant!==target,progress:null,needsProgress:false};
  if(kind==='status' && Object.hasOwn(MOVE_STATUSES,target)) {
    const progress=target==='todo'?0:target==='done'?100:task.progress>0&&task.progress<100?task.progress:null;
    return {changed:statusOf(task.progress)!==MOVE_STATUSES[target],progress,needsProgress:progress===null};
  }
  throw new Error('请选择有效的任务分组');
}
export function statusOf(progress) { return progress === 100 ? '已完成' : progress === 0 ? '未开始' : '进行中'; }
export function localDate(now = new Date()) {
  return `${now.getFullYear()}-${String(now.getMonth()+1).padStart(2,'0')}-${String(now.getDate()).padStart(2,'0')}`;
}
export function calendarMonth(month) {
  if(!/^\d{4}-\d{2}$/.test(month)) throw new Error('无效月份');
  const [year,m]=month.split('-').map(Number);
  if(year<1||year>9999||m<1||m>12) throw new Error('无效月份');
  // setFullYear avoids Date's special handling of years 0–99; never parse UTC date strings.
  const first=new Date(2000,0,1,12);first.setFullYear(year,m-1,1);
  const last=new Date(first);last.setMonth(m,0);
  const offset=(first.getDay()+6)%7,days=last.getDate();
  return {label:`${year} 年 ${m} 月`,offset,days,cells:Array.from({length:Math.ceil((offset+days)/7)*7},(_,i)=>i<offset||i>=offset+days?null:`${month}-${String(i-offset+1).padStart(2,'0')}`)};
}
export function shiftMonth(month,delta) {
  calendarMonth(month);const [y,m]=month.split('-').map(Number);const next=y*12+(m-1)+delta;
  if(next<12||next>9999*12+11)return month;
  return `${String(Math.floor(next/12)).padStart(4,'0')}-${String(next%12+1).padStart(2,'0')}`;
}
export function isOverdue(task, day = localDate()) { return !task.trashed && task.progress < 100 && Boolean(task.plannedDate) && task.plannedDate < day; }
export function selectTasks(tasks, { search = '', scope = 'all', day = localDate() } = {}) {
  const query = search.trim().toLocaleLowerCase();
  return tasks.filter(t => (scope === 'trash' ? t.trashed : !t.trashed)
    && (!query || `${t.title}\n${t.detail}`.toLocaleLowerCase().includes(query))
    && (scope !== 'today' || (t.today && t.progress < 100))
    && (scope !== 'overdue' || isOverdue(t, day))
    && (scope !== 'completed' || t.progress === 100))
    .sort((a,b) => (a.progress === 100) - (b.progress === 100)
      || (a.plannedDate || '9999').localeCompare(b.plannedDate || '9999') || a.title.localeCompare(b.title,'zh-CN') || a.id.localeCompare(b.id));
}
export function metrics(tasks, day = localDate()) {
  const active = tasks.filter(t => !t.trashed);
  const done = active.filter(t => t.progress === 100).length;
  return { total: active.length, done, today: active.filter(t => t.today && t.progress < 100).length,
    overdue: active.filter(t => isOverdue(t,day)).length,
    completionRate: active.length ? Math.round(done / active.length * 100) : 0,
    averageProgress: active.length ? Math.round(active.reduce((n,t) => n + t.progress,0) / active.length) : 0,
    statuses: STATUSES.map(s => ({ label:s, count:active.filter(t => statusOf(t.progress) === s).length })),
    quadrants: Object.entries(QUADRANTS).map(([key,label]) => ({label,count:active.filter(t=>t.quadrant===key).length})) };
}
