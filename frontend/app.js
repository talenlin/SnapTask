const invoke = window.__TAURI__?.core?.invoke;
const listen = window.__TAURI__?.event?.listen;
const appWindow = window.__TAURI__?.window?.getCurrentWindow();
let formalFloatingWrites = false;

const app = document.getElementById('app');
const panel = document.getElementById('panel');
const recordView = document.getElementById('recordView');
const settingsView = document.getElementById('settingsView');
const onboardingView = document.getElementById('onboardingView');
const workInput = document.getElementById('workInput');
const progressInput = document.getElementById('progressInput');
const updateTaskButton = document.getElementById('updateTaskButton');
const createTaskButton = document.getElementById('createTaskButton');
const analysisButtons = [updateTaskButton, createTaskButton];
const formHelp = document.getElementById('formHelp');
const analysisResult = document.getElementById('analysisResult');
const toast = document.getElementById('toast');

let tasks = [];
let floatingTaskContext = null;
let taskRemovalSelection = null;
let taskRemovalBusy = false;
let taskRestoreBusy = false;
let selectedTaskId = null;
let selectedTaskSnapshot = null;
let selectedQuadrant = null;
let temporaryTitle = null;
let temporaryDetail = null;
let selectedPlannedDate = null;
let plannedDateChangeRequested = false;
let originalTaskId = null;
let correctionMode = false;
let editingQueueId = null;
let toastTimer = null;
let toastPriority = 'normal';
let syncInFlight = null;
let writeInFlight = false;
let feishuAuthPollTimer = null;
let activeTaskSource = 'wps';
let onboardingDeferred = false;
let onboardingPage = 'platform';
let onboardingOrigin = 'record';
let workspaceSetupBusy = false;
let workspaceFirstSyncFailed = false;
let onboardingSetup = null;
const activeSourceName = () => activeTaskSource === 'feishu' ? '飞书' : 'WPS';

const labels = {
  iu: '重要且紧急',
  un: '紧急不重要',
  in: '重要不紧急',
  nn: '不重要不紧急'
};

async function call(command, args = {}) {
  if (!invoke) throw new Error('当前不是桌面应用环境');
  return invoke(command, args);
}

function escapeHtml(value) {
  return String(value).replace(/[&<>'"]/g, character => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', "'": '&#39;', '"': '&quot;'
  })[character]);
}

function validate(showError = false) {
  const progress = Number(progressInput.value);
  const valid = workInput.value.trim().length > 0 && progressInput.value !== '' && Number.isInteger(progress) && progress >= 0 && progress <= 100;
  analysisButtons.forEach(button => { button.disabled = !valid; });
  formHelp.hidden = !showError || valid;
  formHelp.textContent = showError && !valid ? '请填写工作描述，并输入 0–100 的整数。' : '';
  return valid;
}

function renderTasks() {
  const taskList = document.getElementById('taskList');
  if (!tasks.length) {
    taskList.innerHTML = '<div class="privacy-note"><strong>没有进行中的任务</strong><p>可以通过上方输入新增临时任务。</p></div>';
    updateSummary();
    return;
  }
  taskList.innerHTML = tasks.map(task => `
    <article class="task-card ${task.today ? 'today' : ''}" data-quadrant="${task.quadrant}">
      <span class="task-title">${escapeHtml(task.title)}</span>
      <span class="badge ${task.quadrant}">${labels[task.quadrant] || '未分类'}</span>
      <span class="task-progress"><span class="track"><i style="--value:${task.progress}%"></i></span><span>${task.progress}%</span></span>
      <span class="task-card-footer">${task.plannedDate ? `<span class="task-date ${dateState(task.plannedDate)}">${escapeHtml(formatPlannedDate(task.plannedDate))}</span>` : ''}<button type="button" class="task-remove-button" data-remove-task="${escapeHtml(task.id)}" aria-label="移除任务：${escapeHtml(task.title)}">移除</button></span>
    </article>
  `).join('');
  updateSummary();
}

function updateSummary() {
  const today = tasks.filter(task => task.today);
  const urgent = tasks.filter(task => task.quadrant === 'iu' || task.quadrant === 'un').length;
  const overall = tasks.length ? Math.round(tasks.reduce((sum, task) => sum + task.progress, 0) / tasks.length) : 0;
  document.getElementById('todaySummary').textContent = `今日 ${today.filter(task => task.progress === 100).length}/${today.length}`;
  document.getElementById('urgentSummary').textContent = `${urgent} 项紧急`;
  document.getElementById('overallProgress').textContent = overall;
  document.getElementById('overallRing').style.setProperty('--progress', `${overall}%`);
}

async function refreshTasks() {
  const view = await call('get_floating_task_view');
  tasks = view.tasks || [];
  floatingTaskContext = view.context || null;
  const restore = document.getElementById('showHiddenFloatingTasks');
  restore.hidden = !(view.hiddenCount > 0);
  restore.textContent = `恢复隐藏（${view.hiddenCount || 0}）`;
  renderTasks();
}

function openTaskRemoval(taskId) {
  if (writeInFlight || taskRemovalBusy || !floatingTaskContext) return;
  const task = tasks.find(t => t.id === taskId);
  if (!task) return;
  taskRemovalSelection = { task: { ...task }, context: { ...floatingTaskContext } };
  if (toastPriority !== 'critical') { clearTimeout(toastTimer); toast.hidden = true; toastPriority = 'normal'; }
  document.getElementById('taskRemovalName').textContent = task.title;
  document.getElementById('taskRemovalTarget').textContent = `${floatingTaskContext.targetLabel} · 记录 ${task.id}`;
  const cloudAllowed = floatingTaskContext.cloudDeleteEnabled === true;
  document.getElementById('deleteFloatingCloudButton').hidden = !cloudAllowed;
  document.getElementById('taskRemovalCloudHelp').hidden = !cloudAllowed;
  document.getElementById('taskRemovalError').hidden = true;
  document.getElementById('taskRemovalDialog').showModal();
}

function closeTaskRemoval() {
  if (taskRemovalBusy) return;
  document.getElementById('taskRemovalDialog').close();
  taskRemovalSelection = null;
}

async function removeFloatingTask(cloud) {
  if (!taskRemovalSelection || taskRemovalBusy || writeInFlight) return;
  const { task, context } = taskRemovalSelection;
  if (cloud && !context.cloudDeleteEnabled) return;
  taskRemovalBusy = true;
  const ids = ['hideFloatingTaskButton', 'deleteFloatingCloudButton', 'cancelTaskRemovalButton'];
  ids.forEach(id => { document.getElementById(id).disabled = true; });
  const action = document.getElementById(cloud ? 'deleteFloatingCloudButton' : 'hideFloatingTaskButton');
  const actionLabel = action.textContent;
  action.textContent = cloud ? '核对飞书记录中…' : '隐藏中…';
  document.getElementById('taskRemovalError').hidden = true;
  try {
    await call(cloud ? 'delete_floating_feishu_task' : 'hide_floating_task', { task, expectedTargetKey: context.key, contextVersion: context.contextVersion });
    document.getElementById('taskRemovalDialog').close();
    taskRemovalSelection = null;
    try {
      await refreshTasks();
      showToast(cloud ? `已从飞书删除并回读确认「${task.title}」` : `已从浮窗隐藏「${task.title}」，云端记录保留`, cloud ? {} : { actionLabel: '恢复显示', onAction: () => restoreHiddenFloatingTasks(context) });
    } catch (refreshError) {
      showToast(`${cloud ? '飞书删除已确认' : '本地隐藏已保存'}，但列表刷新失败：${String(refreshError)}。请点击右上角刷新；不要重复删除。`, { persistent: true });
    }
  } catch (error) {
    const message = String(error);
    if (cloud && /待核实|可能已送达/.test(message)) {
      document.getElementById('taskRemovalDialog').close(); taskRemovalSelection = null;
      showToast(message, { persistent: true, actionLabel: '只读核实', onAction: async () => {
        try { const result = await call('reconcile_floating_feishu_write'); await refreshTasks(); showToast(result.message, { persistent: true }); }
        catch (e) { showToast(String(e), { persistent: true }); }
      } });
    } else {
      document.getElementById('taskRemovalError').textContent = message;
      document.getElementById('taskRemovalError').hidden = false;
    }
  } finally {
    taskRemovalBusy = false;
    action.textContent = actionLabel;
    ids.forEach(id => { document.getElementById(id).disabled = false; });
  }
}

async function restoreHiddenFloatingTasks(context = floatingTaskContext) {
  if (!context || taskRemovalBusy || taskRestoreBusy) return;
  taskRestoreBusy = true;
  try {
    await call('show_hidden_floating_tasks', { expectedTargetKey: context.key, contextVersion: context.contextVersion });
    try {
      await refreshTasks(); showToast('已恢复当前连接中全部隐藏任务的显示，未修改云端记录');
    } catch (refreshError) {
      showToast(`恢复显示已保存，但列表刷新失败：${String(refreshError)}。请点击右上角刷新。`, { persistent: true });
    }
  } catch (e) { showToast(String(e), { persistent: true }); }
  finally { taskRestoreBusy = false; }
}

function setSyncState(kind, text) {
  const dot = document.getElementById('statusDot');
  dot.className = `status-dot${kind === 'pending' ? ' pending' : ''}${kind === 'offline' ? ' offline' : ''}`;
  document.getElementById('syncStatus').textContent = text;
}

function syncedTaskStatus(label, result) {
  const total = result.taskCount;
  const completed = result.completedCount;
  return `${label}已同步 ${total} 项 · 未完成 ${total - completed} · 已完成 ${completed}`;
}

function applyPendingWriteSummary(summary, onlineText = '') {
  if (summary?.uncertain > 0) {
    setSyncState('pending', `${summary.uncertain} 项写入结果待核对 WPS`);
    return true;
  }
  if (summary?.conflict > 0) {
    setSyncState('pending', `${summary.conflict} 项同步冲突，需重新分析`);
    return true;
  }
  if (summary?.pending > 0) {
    setSyncState('offline', `离线，${summary.pending} 项待同步`);
    return true;
  }
  if (summary?.editing > 0) {
    setSyncState('pending', `${summary.editing} 项正在重新分析`);
    return true;
  }
  if (onlineText) setSyncState('online', onlineText);
  return false;
}

async function refreshPendingWriteSummary(onlineText = '') {
  const summary = await call('get_pending_write_summary');
  applyPendingWriteSummary(summary, onlineText);
  return summary;
}

async function syncFromSource(silent = false) {
  if (syncInFlight) return syncInFlight;
  const running = performSourceSync(silent);
  syncInFlight = running;
  try { return await running; }
  finally { if (syncInFlight === running) syncInFlight = null; }
}

async function performSourceSync(silent) {
  try {
    activeTaskSource = await call('get_active_task_source');
    if (activeTaskSource === 'feishu') {
      const config = await call('get_feishu_config');
      if (!config.appId || !config.appToken || !config.tableId || !config.secretConfigured) {
        setSyncState('pending', '飞书未配置 · 本地缓存');
        return false;
      }
    } else {
      const config = await call('get_wps_config');
      if (!config.webhookUrl || !config.tokenConfigured) {
        setSyncState('pending', 'WPS 未配置 · 本地缓存');
        return false;
      }
    }
    const sourceLabel = activeTaskSource === 'feishu' ? '飞书' : 'WPS';
    setSyncState('pending', `${sourceLabel}后台同步中`);
    const beforeQueue = await call('get_pending_write_summary');
    const result = await call('sync_active_tasks');
    await refreshTasks();
    const afterQueue = await refreshPendingWriteSummary(syncedTaskStatus(sourceLabel, result));
    const syncedQueued = Math.max(0, (beforeQueue.pending || 0) - (afterQueue.pending || 0) - (afterQueue.conflict || 0) - (afterQueue.uncertain || 0));
    if (!silent) showToast(syncedQueued > 0
      ? `已同步 ${syncedQueued} 项离线记录，并刷新「${result.sourceName}」`
      : `已从${sourceLabel}「${result.sourceName}」同步 ${result.taskCount} 项任务`);
    return true;
  } catch (error) {
    try {
      const summary = await call('get_pending_write_summary');
      if (!applyPendingWriteSummary(summary)) setSyncState('offline', '离线，尚未同步');
    } catch (_) {
      setSyncState('offline', '离线，尚未同步');
    }
    if (!silent) showToast(`同步失败：${String(error)}`);
    return false;
  }
}

async function expand() {
  app.className = 'app expanded';
  panel.setAttribute('aria-hidden', 'false');
  document.getElementById('expandButton').setAttribute('aria-expanded', 'true');
  await call('set_expanded', { expanded: true });
  workInput.focus({ preventScroll: true });
  if (!onboardingDeferred && await needsFirstUseOnboarding()) {
    workInput.blur();
    setSyncState('pending', '首次使用 · 待连接任务空间');
    await showOnboarding(onboardingSetup?.status?.hasPendingOperation ? 'feishu' : 'platform');
    return;
  }
  syncFromSource(true).then(synced => {
    if (!synced) return refreshTasks();
  }).catch(error => console.warn('后台同步未完成:', error));
}

async function collapse() {
  showRecordView();
  app.className = 'app collapsed';
  panel.setAttribute('aria-hidden', 'true');
  document.getElementById('expandButton').setAttribute('aria-expanded', 'false');
  await call('set_expanded', { expanded: false });
}

function showToast(message, undoableOrOptions = false) {
  const options = typeof undoableOrOptions === 'boolean'
    ? { undoable: undoableOrOptions }
    : undoableOrOptions;
  const priority = options.priority || (options.persistent ? 'critical' : 'normal');
  if (!toast.hidden && toastPriority === 'critical' && priority !== 'critical') return;
  clearTimeout(toastTimer);
  toastPriority = priority;
  const action = options.undoable
    ? '<button id="undoButton" type="button">撤销</button>'
    : options.actionLabel
      ? `<button id="toastActionButton" type="button">${escapeHtml(options.actionLabel)}</button>`
      : '';
  const close = options.persistent ? '<button id="toastCloseButton" type="button" aria-label="关闭提示">关闭</button>' : '';
  toast.innerHTML = `<span>${escapeHtml(message)}</span><span class="toast-actions">${action}${close}</span>`;
  toast.hidden = false;
  if (options.undoable) {
    const source = activeTaskSource;
    document.getElementById('undoButton').addEventListener('click', () => {
      if (source !== activeTaskSource) { showToast('数据源已变化，旧撤销入口已失效', {persistent:true}); return; }
      return options.onUndo ? options.onUndo() : undoLatest();
    });
  }
  if (options.actionLabel && options.onAction) {
    document.getElementById('toastActionButton').addEventListener('click', options.onAction);
  }
  if (options.persistent) {
    document.getElementById('toastCloseButton').addEventListener('click', () => {
      toast.hidden = true;
      toastPriority = 'normal';
    });
    return;
  }
  toastTimer = setTimeout(() => {
    toast.hidden = true;
    toastPriority = 'normal';
  }, options.undoable ? 10000 : 3500);
}

function hideAnalysis() {
  analysisResult.hidden = true;
  recordView.classList.remove('confirming');
  selectedTaskId = null;
  selectedTaskSnapshot = null;
  selectedQuadrant = null;
  temporaryTitle = null;
  temporaryDetail = null;
  selectedPlannedDate = null;
  plannedDateChangeRequested = false;
  originalTaskId = null;
  correctionMode = false;
}

function localIsoDate(date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function extractExplicitPlannedDate(text) {
  const source = String(text);
  const full = source.match(/(?:^|[^\d])(20\d{2})[-/.年](0?[1-9]|1[0-2])[-/.月](0?[1-9]|[12]\d|3[01])日?(?!\d)/);
  if (full) return `${full[1]}-${String(full[2]).padStart(2, '0')}-${String(full[3]).padStart(2, '0')}`;
  const today = new Date();
  if (/后天/.test(source)) { today.setDate(today.getDate() + 2); return localIsoDate(today); }
  if (/明天|明日/.test(source)) { today.setDate(today.getDate() + 1); return localIsoDate(today); }
  const monthDay = source.match(/(?:^|[^\d])(0?[1-9]|1[0-2])月(0?[1-9]|[12]\d|3[01])日?(?!\d)/);
  if (monthDay) {
    let year = today.getFullYear();
    const candidate = new Date(year, Number(monthDay[1]) - 1, Number(monthDay[2]));
    const start = new Date(today.getFullYear(), today.getMonth(), today.getDate());
    if (candidate < start) year += 1;
    return `${year}-${String(monthDay[1]).padStart(2, '0')}-${String(monthDay[2]).padStart(2, '0')}`;
  }
  const weekday = source.match(/下周([一二三四五六日天])/);
  if (weekday) {
    const number = '一二三四五六日'.indexOf(weekday[1] === '天' ? '日' : weekday[1]) + 1;
    const current = today.getDay() || 7;
    today.setDate(today.getDate() + (7 - current) + number);
    return localIsoDate(today);
  }
  return null;
}

function hasPlannedDateChangeIntent(text) {
  return /延期|推迟|延后|延至|改到|调整.{0,6}(完成|截止|日期|时间)|截止.{0,4}(改|调整)|提前到|提前至/.test(String(text));
}

function formatPlannedDate(value) {
  const [year, month, day] = String(value).split('-').map(Number);
  if (!year || !month || !day) return value;
  const today = new Date();
  const target = new Date(year, month - 1, day);
  const delta = Math.round((target - new Date(today.getFullYear(), today.getMonth(), today.getDate())) / 86400000);
  if (delta < 0) return `逾期 ${Math.abs(delta)} 天`;
  if (delta === 0) return '今天到期';
  if (delta === 1) return '明天到期';
  return `${month}月${day}日`;
}

function dateState(value) {
  const target = new Date(`${value}T00:00:00`);
  const today = new Date();
  const delta = Math.round((target - new Date(today.getFullYear(), today.getMonth(), today.getDate())) / 86400000);
  return delta < 0 ? 'overdue' : delta <= 3 ? 'soon' : '';
}

function plannedDateEditor(value, required, current = '') {
  const hint = required
    ? current ? `当前 ${formatPlannedDate(current)}；请选择调整后的日期。` : '请选择调整后的计划完成日期。'
    : '可选；没有明确计划时可以留空。';
  return `<label class="planned-date-field"><span>计划完成日期${required ? '' : '（可选）'}</span><input id="plannedDateInput" type="date" value="${escapeHtml(value || '')}" ${required ? 'required' : ''} /><p>${escapeHtml(hint)}</p></label>`;
}

function habitLearningControl(checked = false) {
  return `<div class="habit-learning"><label><input id="rememberHabitInput" type="checkbox" ${checked ? 'checked' : ''} />记住这个说法</label><input id="habitPhraseInput" type="text" value="${escapeHtml(workInput.value.trim())}" aria-label="需要记住的说法" /><p>只保存在本机；以后优先按这个对应关系理解。</p></div>`;
}

function suggestTaskTypePhrase(description, title) {
  const source = [...String(description).replace(/\d{4}-?\d+[A-Z]{0,3}/gi, '')];
  const target = [...String(title)];
  const ignored = new Set(['完成', '完成了', '今天', '任务', '处理', '提交', '递交', '进行']);
  let best = '';
  for (let start = 0; start < target.length; start += 1) {
    for (let end = start + 2; end <= target.length; end += 1) {
      const candidate = target.slice(start, end).join('').trim();
      if (candidate.length > best.length && !ignored.has(candidate) && source.join('').includes(candidate)) best = candidate;
    }
  }
  return best || String(title).trim().slice(0, 16);
}

function taskTypeLearningControl(title, checked = false, suggestedPhrase = '') {
  const phrase = suggestedPhrase || suggestTaskTypePhrase(workInput.value, title);
  return `<div class="habit-learning task-type-learning"><label><input id="rememberHabitInput" type="checkbox" data-kind="task_type" ${checked ? 'checked' : ''} />记住为固定任务类型</label><input id="habitPhraseInput" type="text" value="${escapeHtml(phrase)}" aria-label="固定任务类型识别关键词" /><p id="taskTypeLearningHint">以后出现“${escapeHtml(phrase)}”时，默认新增为“${escapeHtml(title)}”。</p></div>`;
}

function candidateButtons(candidates) {
  return candidates.map(task => `<button class="candidate-button" data-task-id="${escapeHtml(task.id)}" data-title="${escapeHtml(task.title)}" data-progress="${task.progress}" data-status="${escapeHtml(task.status || '')}" data-planned-date="${escapeHtml(task.plannedDate || '')}" type="button" aria-pressed="false">${escapeHtml(task.title)} · 当前 ${task.progress}%${task.plannedDate ? ` · ${escapeHtml(formatPlannedDate(task.plannedDate))}` : ''}</button>`).join('');
}

function renderMatch(result) {
  selectedTaskId = result.task.id;
  selectedTaskSnapshot = { title: result.task.title, progress: result.task.progress, status: result.task.status };
  selectedTaskSnapshot.plannedDate = result.task.plannedDate || null;
  originalTaskId = result.task.id;
  const rollback = result.next_progress < result.task.progress ? '<p class="warning">注意：这是进度回退。</p>' : '';
  const complete = result.next_progress === 100 ? '<p class="warning">确认后将同时标记为已完成。</p>' : '';
  plannedDateChangeRequested = hasPlannedDateChangeIntent(workInput.value);
  selectedPlannedDate = plannedDateChangeRequested ? extractExplicitPlannedDate(workInput.value) : null;
  const datePart = plannedDateChangeRequested
    ? plannedDateEditor(selectedPlannedDate, true, result.task.plannedDate || '')
    : result.task.plannedDate ? `<p class="planned-date-readonly">计划完成：${escapeHtml(formatPlannedDate(result.task.plannedDate))} · 保持不变</p>` : '';
  analysisResult.innerHTML = `<h3>将更新「${escapeHtml(result.task.title)}」</h3><p>${result.task.progress}% → ${result.next_progress}%</p>${analysisMeta(result)}${rollback}${complete}${datePart}<div class="result-actions"><button class="secondary-button" data-action="correct" type="button">不是这个任务</button><button class="secondary-button" data-action="cancel" type="button">修改</button><button class="primary-button" data-action="confirm" type="button" ${plannedDateChangeRequested && !selectedPlannedDate ? 'disabled' : ''}>确认写入</button></div>`;
}

function renderAmbiguous(result) {
  plannedDateChangeRequested = hasPlannedDateChangeIntent(workInput.value);
  selectedPlannedDate = plannedDateChangeRequested ? extractExplicitPlannedDate(workInput.value) : null;
  const datePart = plannedDateChangeRequested ? plannedDateEditor(selectedPlannedDate, true) : '';
  analysisResult.innerHTML = `<h3>请选择要更新的任务</h3><p>本次不会新增任务。</p>${analysisMeta(result)}<div class="candidate-list">${candidateButtons(result.candidates)}</div>${datePart}${habitLearningControl(false)}<div class="result-actions"><button class="secondary-button" data-action="cancel" type="button">修改描述</button><button class="primary-button" data-action="confirm" type="button" disabled>确认选择</button></div>`;
}

function renderTemporary(result) {
  temporaryTitle = result.suggested_title;
  temporaryDetail = result.suggested_detail || '';
  selectedTaskId = 'temporary';
  selectedTaskSnapshot = null;
  selectedQuadrant = result.suggested_quadrant || null;
  plannedDateChangeRequested = true;
  selectedPlannedDate = extractExplicitPlannedDate(workInput.value);
  const learned = result.analysis_source === 'habit';
  analysisResult.innerHTML = `<h3>将新增临时任务</h3><label class="temporary-title-field"><span>任务名称</span><input id="temporaryTitleInput" type="text" maxlength="40" value="${escapeHtml(temporaryTitle)}" /></label><label class="temporary-title-field temporary-detail-field"><span>任务详情（可修改）</span><textarea id="temporaryDetailInput" maxlength="240" placeholder="补充工作范围、交付物或下一步">${escapeHtml(temporaryDetail)}</textarea></label><p>进度 ${result.next_progress}%</p>${analysisMeta(result)}${plannedDateEditor(selectedPlannedDate, false)}<div class="candidate-list quadrant-list">${Object.entries(labels).map(([key, label]) => `<button class="candidate-button" data-quadrant="${key}" type="button" aria-pressed="${String(selectedQuadrant === key)}">${label}</button>`).join('')}</div>${learned ? '' : taskTypeLearningControl(temporaryTitle, false, result.suggested_phrase)}<div class="result-actions"><button class="secondary-button" data-action="cancel" type="button">修改描述</button><button class="primary-button" data-action="confirm" type="button" ${selectedQuadrant ? '' : 'disabled'}>确认新增</button></div>`;
}

function analysisMeta(result) {
  const source = result.analysis_source === 'ai' ? 'AI 判断' : result.analysis_source === 'habit' ? '习惯表' : '本地规则';
  const confidence = Number.isFinite(result.confidence) ? ` · 置信度 ${result.confidence}%` : '';
  const retrieval = result.retrieval;
  let saving = '';
  if (retrieval?.estimatedFullContextTokens > 0) {
    const percent = Math.max(0, Math.round((1 - retrieval.estimatedContextTokens / retrieval.estimatedFullContextTokens) * 100));
    saving = ` · 本地筛选 ${retrieval.selectedTasks}/${retrieval.totalTasks} 项`;
    if (retrieval.selectedHabits) saving += ` + ${retrieval.selectedHabits} 条习惯`;
    saving += ` · 上下文约省 ${percent}%`;
  }
  return `<p class="analysis-meta">${source}${confidence}${saving}</p>`;
}

async function showCorrectionPicker() {
  const candidates = await call('list_task_candidates');
  correctionMode = true;
  selectedTaskId = null;
  selectedTaskSnapshot = null;
  selectedQuadrant = null;
  analysisResult.innerHTML = `<h3>请选择正确任务</h3><p>更正后可把这个说法记入习惯表。</p><div class="candidate-list">${candidateButtons(candidates)}<button class="candidate-button" data-task-id="temporary" type="button" aria-pressed="false">它是新的临时任务</button></div>${habitLearningControl(true)}<div class="result-actions"><button class="secondary-button" data-action="cancel" type="button">取消</button><button class="primary-button" data-action="confirm" type="button" disabled>确认更正</button></div>`;
}

async function analyze(event) {
  event.preventDefault();
  if (!validate(true)) return;
  const intent = event.submitter?.dataset.analysisIntent || 'update';
  const activeButton = intent === 'create' ? createTaskButton : updateTaskButton;
  analysisButtons.forEach(button => { button.disabled = true; });
  activeButton.textContent = intent === 'create' ? '正在整理…' : '正在匹配…';
  try {
    if (intent === 'update' && activeTaskSource === 'feishu') {
      hideAnalysis();
      activeButton.textContent = '正在刷新飞书…';
      if (!(await syncFromSource(true))) {
        throw new Error('未能刷新飞书，未进行任务匹配。请检查连接后重试；不会使用旧缓存继续更新。');
      }
      activeButton.textContent = '正在匹配…';
    }
    const response = await call('analyze_update', { description: workInput.value.trim(), progress: Number(progressInput.value), intent });
    const result = response.analysis;
    result.retrieval = response.retrieval;
    hideAnalysis();
    if (result.kind === 'match') renderMatch(result);
    if (result.kind === 'ambiguous') renderAmbiguous(result);
    if (result.kind === 'temporary') renderTemporary(result);
    analysisResult.hidden = false;
    recordView.classList.add('confirming');
  } catch (error) {
    showToast(String(error), {
      persistent: true,
      actionLabel: '重试分析',
      onAction: () => {
        toast.hidden = true;
        toastPriority = 'normal';
        document.getElementById('progressForm').requestSubmit(activeButton);
      }
    });
  } finally {
    updateTaskButton.textContent = '更新已有任务';
    createTaskButton.textContent = '新增任务';
    validate();
  }
}

async function confirm() {
  if ((!selectedTaskId && !selectedQuadrant) || writeInFlight) return;
  const submittedSource = typeof activeTaskSource !== 'undefined' ? activeTaskSource : null;
  const submittedSourceName = activeSourceName();
  const button = analysisResult.querySelector('[data-action="confirm"]');
  const titleInput = document.getElementById('temporaryTitleInput');
  if (titleInput) {
    temporaryTitle = titleInput.value.trim();
    if (!temporaryTitle) {
      showToast('请填写任务名称');
      titleInput.focus();
      return;
    }
  }
  if (selectedTaskId !== 'temporary' && plannedDateChangeRequested && !selectedPlannedDate) {
    showToast('这次是日期调整，请先选择新的计划完成日期');
    document.getElementById('plannedDateInput')?.focus();
    return;
  }
  const changePlannedDate = selectedTaskId === 'temporary'
    ? Boolean(selectedPlannedDate)
    : plannedDateChangeRequested && selectedPlannedDate !== (selectedTaskSnapshot?.plannedDate || null);
  writeInFlight = true;
  button.disabled = true;
  button.textContent = '正在写入…';
  const rememberCheckbox = document.getElementById('rememberHabitInput');
  const habit = rememberCheckbox?.checked ? {
    phrase: document.getElementById('habitPhraseInput').value.trim(),
    kind: rememberCheckbox.dataset.kind === 'task_type'
      ? 'task_type'
      : correctionMode || (originalTaskId && originalTaskId !== selectedTaskId) ? 'correction' : 'alias'
  } : null;
  let rememberedBeforeWrite = false;
  try {
    if (habit && habit.kind !== 'task_type' && selectedTaskId && selectedTaskId !== 'temporary') {
      try {
        await call('remember_task_mapping', {
          taskId: selectedTaskId,
          expectedTitle: selectedTaskSnapshot?.title || '',
          habit
        });
        rememberedBeforeWrite = true;
      } catch (_) {
        // 写入成功后仍会由后端再次尝试保存，不阻断本次进度提交。
      }
    }
    const result = await (typeof formalFloatingWrites !== 'undefined' && formalFloatingWrites && activeTaskSource === 'feishu' ? submitFormalFloatingFeishu : args => call('confirm_update', args))({
      input: {
        taskId: selectedTaskId === 'temporary' ? null : selectedTaskId,
        temporaryTitle,
        description: selectedTaskId === 'temporary' ? (temporaryDetail || workInput.value.trim()) : workInput.value.trim(),
        progress: Number(progressInput.value),
        quadrant: selectedQuadrant,
        today: /今天|今晚|下班前/.test(workInput.value),
        changePlannedDate,
        plannedDate: changePlannedDate ? selectedPlannedDate : null,
        expectedTitle: selectedTaskSnapshot?.title ?? null,
        expectedProgress: selectedTaskSnapshot?.progress ?? null,
        expectedStatus: selectedTaskSnapshot?.status ?? null,
        expectedPlannedDateKnown: Boolean(selectedTaskSnapshot),
        expectedPlannedDate: selectedTaskSnapshot?.plannedDate ?? null
      },
      habit: rememberedBeforeWrite ? null : habit,
      replacesQueueId: editingQueueId
    });
    workInput.value = '';
    progressInput.value = '';
    hideAnalysis();
    validate();
    if (result.queued) {
      editingQueueId = null;
      const summary = await refreshPendingWriteSummary();
      setSyncState('offline', `离线，${summary.pending} 项待同步`);
      showToast('当前无法连接 WPS，已安全加入待同步队列');
      return;
    }
    await refreshTasks();
    editingQueueId = null;
    const sourceName = submittedSourceName;
    const sameSource = submittedSource === null || submittedSource === activeTaskSource;
    if (sameSource) setSyncState('online', `${sourceName} 已写入并校验`);
    showToast(result.habitWarning
      ? `${sourceName} 已写入，但习惯保存失败：${result.habitWarning}`
      : result.habitSaved || rememberedBeforeWrite
        ? `已写入「${result.task?.title || '任务'}」，并记住这次对应关系`
        : `已写入${sourceName}并回读确认「${result.task?.title || '任务'}」`, {
      undoable: sameSource,
      onUndo: result.undoReceipt ? () => undoLatest(result.undoReceipt) : null,
      persistent: submittedSource === 'feishu',
      priority: 'normal'
    });
  } catch (error) {
    const message = String(error);
    const sourceName = activeSourceName();
    if (message.includes('已取消')) { setSyncState('pending', '已取消，未发送写入'); showToast(message); return; }
    const formalPending = typeof formalFloatingWrites !== 'undefined' && formalFloatingWrites && activeTaskSource === 'feishu' && /待核实|可能已送达/.test(message);
    const conflict = message.includes(`${sourceName}冲突`) || message.includes('任务标识对应关系已变化');
    const uncertain = message.includes('核对与撤销记录') || message.includes('写入请求可能已送达');
    const blocked = message.includes('写入能力校验') || message.includes('写入尚未开放');
    const permissionDenied = message.includes('代码 91403') || message.includes('记录写入权限');
    setSyncState('pending', permissionDenied ? '飞书权限不足，请核对表格编辑授权' : blocked
      ? `${sourceName}写入尚未开放`
      : conflict
      ? `${sourceName} 已变化，请重新分析`
      : formalPending ? '写入结果待核实'
      : uncertain
        ? '写入结果待核对，可撤销'
        : `写入未确认，请核对${sourceName}`);
    const actionLabel = formalPending ? '只读核实本次写入' : uncertain
      ? '撤销本次可能写入'
      : conflict
        ? '按最新数据重新分析'
        : null;
    const onAction = formalPending ? async () => { try { const result = await call('reconcile_floating_feishu_write'); await refreshTasks(); showToast(result.message, {persistent:true,undoable:Boolean(result.undoReceipt),onUndo:()=>undoLatest(result.undoReceipt)}); } catch (e) {showToast(String(e), {persistent:true});} } : uncertain
      ? async () => {
          toast.hidden = true;
          toastPriority = 'normal';
          await undoLatest();
        }
      : conflict
        ? async () => {
            toast.hidden = true;
            toastPriority = 'normal';
            hideAnalysis();
            if (!(await syncFromSource(true))) {
              showToast(`无法刷新${sourceName}，尚未重新分析；请检查网络后重试`, { persistent: true });
              return;
            }
            document.getElementById('progressForm').requestSubmit(updateTaskButton);
          }
        : null;
    showToast(`${rememberedBeforeWrite ? '任务未写入，但已记住对应关系。' : ''}写入失败：${message}`, {
      persistent: true,
      actionLabel,
      onAction
    });
  } finally {
    writeInFlight = false;
    if (!analysisResult.hidden) {
      button.disabled = false;
      button.textContent = selectedTaskId === 'temporary' || !selectedTaskId ? '确认新增' : '确认写入';
    }
  }
}

async function undoLatest(receipt = null) {
  try {
    const sourceName = activeSourceName();
    setSyncState('pending', `正在撤销${sourceName}写入…`);
    const formal = typeof formalFloatingWrites !== 'undefined' && formalFloatingWrites && activeTaskSource === 'feishu';
    if (receipt && !formal) throw new Error('数据源已变化，旧撤销入口已失效');
    if (formal && !receipt) throw new Error('缺少本次写入的撤销凭据，请先只读核实');
    const result = await call(formal ? 'undo_floating_feishu_write' : 'undo_latest', formal ? {receipt} : undefined);
    await refreshTasks();
    if (result.undone) setSyncState('online', `${sourceName}撤销已回读确认`);
    showToast(result.undone ? `已在${sourceName}撤销「${result.taskTitle}」` : '没有可撤销的云端写入');
  } catch (error) {
    setSyncState('pending', `撤销未确认，请核对${activeSourceName()}`);
    showToast(`撤销失败：${String(error)}`);
  }
}

function renderFeishuConnectionConfig(config) {
  document.getElementById('feishuAppIdInput').value = config.appId || '';
  document.getElementById('feishuAppTokenInput').value = config.appToken || '';
  document.getElementById('feishuTableIdInput').value = config.tableId || '';
  document.getElementById('feishuAppSecretInput').value = '';
  document.getElementById('feishuAppSecretInput').placeholder = config.secretConfigured ? '密钥已安全保存；留空表示不修改' : '粘贴 App Secret';
  document.getElementById('feishuConfigBadge').textContent = config.appId && config.appToken && config.tableId && config.secretConfigured ? '已保存' : '未配置';
}

async function showSettings() {
  onboardingView.hidden = true;
  recordView.hidden = true;
  settingsView.hidden = false;
  try {
    const [enabled, config, feishuConfig, source, aiConfig, habits, queueItems, feishuSetup] = await Promise.all([
      call('get_autostart'),
      call('get_wps_config'),
      call('get_feishu_config'),
      call('get_active_task_source'),
      call('get_ai_config'),
      call('list_habits'),
      call('list_pending_write_items'),
      call('get_task_workspace_setup_status')
    ]);
    activeTaskSource = source;
    document.getElementById('activeSourceSelect').value = source;
    document.getElementById('activeSourceBadge').textContent = source === 'feishu' ? '飞书' : 'WPS';
    document.getElementById('autostartToggle').setAttribute('aria-checked', String(enabled));
    document.getElementById('wpsWebhookInput').value = config.webhookUrl || '';
    document.getElementById('wpsSheetInput').value = config.sheetName || '任务清单';
    document.getElementById('wpsTokenInput').value = '';
    document.getElementById('wpsTokenInput').placeholder = config.tokenConfigured ? '令牌已安全保存；留空表示不修改' : '粘贴脚本令牌';
    document.getElementById('wpsConfigBadge').textContent = config.webhookUrl && config.tokenConfigured ? '已保存' : '未配置';
    renderFeishuConnectionConfig(feishuConfig);
    if (feishuSetup?.workspaceUrl) document.getElementById('feishuTableUrlInput').value = feishuSetup.workspaceUrl;
    renderFeishuProvisionStatus(feishuSetup, feishuConfig, source);
    renderFeishuAuthorizationReady(feishuSetup?.userAuthorized);
    document.getElementById('aiProviderSelect').value = aiConfig.provider || 'deepseek';
    document.getElementById('aiEndpointInput').value = aiConfig.provider === 'generic' ? (aiConfig.endpoint || '') : '';
    const modelSelect = document.getElementById('aiModelSelect');
    modelSelect.innerHTML = aiConfig.model
      ? `<option value="${escapeHtml(aiConfig.model)}">${escapeHtml(aiConfig.model)}</option>`
      : '<option value="">请先读取模型</option>';
    modelSelect.disabled = !aiConfig.model;
    document.getElementById('aiKeyInput').value = '';
    document.getElementById('aiKeyInput').placeholder = aiConfig.apiKeyConfigured ? '密钥已安全保存；留空表示不修改' : '粘贴模型 API Key';
    document.getElementById('aiEnabledToggle').setAttribute('aria-checked', String(aiConfig.enabled));
    document.getElementById('aiConfigBadge').textContent = aiConfig.endpoint && aiConfig.model && aiConfig.apiKeyConfigured ? (aiConfig.enabled ? '已启用' : '已停用') : '未配置';
    updateAiProviderUi(false);
    renderHabits(habits);
    renderQueueItems(queueItems);
  } catch (error) {
    showToast(`无法读取设置：${String(error)}`);
  }
}

function hasSavedTaskConnection(wps, feishu) {
  // Keep incomplete/expired existing connections in settings; never treat them as a fresh install.
  return Boolean(wps?.webhookUrl || feishu?.appToken || feishu?.tableId);
}

async function needsFirstUseOnboarding() {
  try {
    const [wps, feishu, status] = await Promise.all([
      call('get_wps_config'), call('get_feishu_config'), call('get_task_workspace_setup_status')
    ]);
    onboardingSetup = { status, config: feishu, source: activeTaskSource };
    return Boolean(status?.hasPendingOperation) || !hasSavedTaskConnection(wps, feishu);
  } catch (error) {
    // A read failure must not invite the user to create another remote space.
    showToast(`无法检查已有连接，请在设置中重试：${String(error)}`);
    return false;
  }
}

async function showOnboarding(page = 'platform') {
  onboardingOrigin = settingsView.hidden ? 'record' : 'settings';
  onboardingPage = page;
  recordView.hidden = true;
  settingsView.hidden = true;
  onboardingView.hidden = false;
  onboardingView.scrollTop = 0;
  renderOnboarding();
  try {
    await refreshFeishuProvisionStatus();
  } catch (error) {
    document.getElementById('onboardingProgress').textContent = `无法读取连接状态：${String(error)}。请返回设置后重试；不会开始创建。`;
    document.getElementById('createFeishuWorkspaceButton').disabled = true;
    showToast('无法读取连接状态，请返回设置后重试');
  }
}

function renderOnboarding() {
  const { status, config, result } = onboardingSetup || {};
  const connected = Boolean(config?.appToken);
  const pending = Boolean(status?.hasPendingOperation);
  const repairNeeded = connected && !(config?.appId && config?.tableId && config?.secretConfigured);
  const completed = connected && !repairNeeded && !pending && !workspaceSetupBusy;
  const step = workspaceSetupBusy || pending || repairNeeded ? 3 : completed ? 4 : status?.userAuthorized ? 2 : 1;
  document.getElementById('onboardingPlatformPanel').hidden = onboardingPage !== 'platform';
  document.getElementById('onboardingFeishuPanel').hidden = onboardingPage !== 'feishu';
  document.getElementById('feishuProvisionCard').hidden = onboardingPage !== 'feishu';
  document.querySelectorAll('#onboardingSteps li').forEach((item, index) => {
    item.classList.toggle('current', index === step - 1);
    item.classList.toggle('done', index < step - 1);
    if (index === step - 1) item.setAttribute('aria-current', 'step');
    else item.removeAttribute('aria-current');
  });
  const progress = document.getElementById('onboardingProgress');
  progress.setAttribute('aria-busy', String(workspaceSetupBusy));
  progress.textContent = workspaceSetupBusy ? '正在创建并检查任务空间，请稍候。关闭窗口后可继续上次初始化，请勿重复创建。'
    : pending ? (status.needsReconciliation ? '需要先核对副本，再继续连接。' : '上次初始化未完成，可从保存的步骤继续。')
    : repairNeeded ? '已有连接配置不完整，请在高级连接中补齐；不会创建新副本。'
    : completed ? (workspaceFirstSyncFailed ? '空间已连接，浮窗首次同步尚未完成；本地工作台尚未同步。' : '任务空间已连接；本地工作台同步需另行预览和确认。')
    : status?.userAuthorized ? '账号已连接，请为新任务空间命名。' : '先打开飞书确认账号，再生成授权链接；无需填写 App ID 或表格编号。';
  document.getElementById('feishuProvisionTitle').textContent = completed ? '任务空间已连接' : '准备飞书任务空间';
  document.getElementById('feishuProvisionHelp').hidden = workspaceSetupBusy || completed;
  document.getElementById('feishuProvisionBadge').hidden = workspaceSetupBusy;
  document.getElementById('feishuWorkspaceProfile').hidden = step !== 2;
  document.getElementById('createFeishuWorkspaceButton').hidden = step !== 2;
  document.getElementById('authorizeFeishuUserButton').hidden = workspaceSetupBusy || (status?.userAuthorized && !pending);
  document.getElementById('feishuAccountGuide').hidden = document.getElementById('authorizeFeishuUserButton').hidden;
  document.getElementById('feishuAuthorizationHelp').hidden = step !== 1;
  if (status?.userAuthorized && !workspaceSetupBusy) document.getElementById('feishuAuthorizationPanel').hidden = true;
  document.getElementById('onboardingRetrySyncButton').hidden = !completed || !workspaceFirstSyncFailed;
  document.getElementById('onboardingFinishButton').hidden = !completed;
  document.getElementById('onboardingWriteNotice').hidden = !completed;
  document.getElementById('onboardingRepairConnectionButton').hidden = !repairNeeded || workspaceSetupBusy;
  document.getElementById('resumeFeishuWorkspaceButton').disabled = workspaceSetupBusy;
  document.getElementById('reconcileFeishuWorkspaceButton').disabled = workspaceSetupBusy;
  document.getElementById('onboardingBackButton').disabled = workspaceSetupBusy;
  document.getElementById('settingsButton').disabled = workspaceSetupBusy;
  document.getElementById('feishuWorkspaceNameInput').disabled = workspaceSetupBusy;
  document.getElementById('feishuWorkspaceFolderLinkInput').disabled = workspaceSetupBusy;
  // Keep actionable backend errors near their recovery buttons, not only in a transient toast.
  if (result?.state && result.state !== 'completed') progress.textContent += ' 请按下方提示处理。';
}

async function openAdvancedConnection(platform = 'feishu') {
  await showSettings();
  const details = document.getElementById(platform === 'wps' ? 'wpsAdvancedConnection' : 'feishuAdvancedConnection');
  details.open = true;
  details.scrollIntoView({ block: 'start' });
  details.querySelector('summary').focus();
}

async function leaveOnboarding() {
  if (workspaceSetupBusy) return;
  if (onboardingPage === 'feishu' && !onboardingSetup?.status?.hasPendingOperation && !onboardingSetup?.config?.appToken) {
    onboardingPage = 'platform';
    renderOnboarding();
  } else if (onboardingOrigin === 'settings') await showSettings();
  else {
    onboardingDeferred = true;
    showRecordView();
  }
}

function renderFeishuProvisionStatus(status, config, source, result = null) {
  onboardingSetup = { status, config, source, result };
  const badge = document.getElementById('feishuProvisionBadge');
  const help = document.getElementById('feishuProvisionHelp');
  const createButton = document.getElementById('createFeishuWorkspaceButton');
  const recoveryActions = document.getElementById('feishuProvisionRecoveryActions');
  const resumeButton = document.getElementById('resumeFeishuWorkspaceButton');
  const reconcileButton = document.getElementById('reconcileFeishuWorkspaceButton');
  const resultText = document.getElementById('feishuProvisionResult');
  const workspaceLink = document.getElementById('feishuWorkspaceLink');
  renderFeishuAuthorizationReady(status?.userAuthorized);
  const hasExistingConnection = Boolean(config?.appToken);
  const needsReconciliation = Boolean(status?.needsReconciliation);
  const hasPendingOperation = Boolean(status?.hasPendingOperation);
  const canCreate = Boolean(status?.templateAvailable && status?.userAuthorized)
    && !hasPendingOperation && !hasExistingConnection && source !== 'feishu';

  createButton.disabled = !canCreate;
  recoveryActions.hidden = !hasPendingOperation;
  resumeButton.hidden = !hasPendingOperation || needsReconciliation || !status?.templateAvailable;
  reconcileButton.hidden = !needsReconciliation;

  if (hasPendingOperation && needsReconciliation) {
    badge.textContent = '待核对';
    help.textContent = `上次初始化在“${status.workspaceName || '飞书任务空间'}”阶段中断。先到飞书确认是否已出现副本，再核对结果；不会自动重复复制。`;
  } else if (!status?.templateAvailable) {
    badge.textContent = '模板待接入';
    help.textContent = '飞书任务模板未就绪，请更新应用或联系维护者。当前不会开始任何云端操作。';
  } else if (hasPendingOperation) {
    badge.textContent = '可继续';
    help.textContent = `检测到未完成的“${status.workspaceName || '飞书任务空间'}”初始化，可从保存的检查点继续。`;
  } else if (!status?.userAuthorized) {
    badge.textContent = '需要授权';
    help.textContent = '连接飞书账号后，应用会按当前账号已有权限访问多维表格；授权完成后即可复制并连接空白个人任务模板。';
  } else if (hasExistingConnection || source === 'feishu') {
    badge.textContent = '已连接';
    help.textContent = '已检测到飞书任务空间。为避免重复创建，请继续使用当前连接或先检查下方配置。';
  } else {
    badge.textContent = '可以创建';
    help.textContent = '模板和用户授权均已就绪。创建前请确认名称；复制完成后会自动发现任务表并保存连接。';
  }

  const visibleResult = result || null;
  if (visibleResult?.message) {
    resultText.hidden = false;
    resultText.textContent = visibleResult.message;
  } else if (visibleResult?.state) {
    resultText.hidden = false;
    const states = { completed: '任务空间已创建并连接。', recoverableFailure: '初始化未完成，请按提示继续。', needsReconciliation: '复制结果需要核对，请先确认是否已有副本。' };
    resultText.textContent = states[visibleResult.state] || '初始化未完成，请查看下方恢复操作。';
  } else {
    resultText.hidden = true;
    resultText.textContent = '';
  }

  const url = visibleResult?.workspaceUrl || status?.workspaceUrl || '';
  try {
    const parsed = new URL(url);
    if (parsed.protocol === 'https:' && /(^|\.)feishu\.cn$|(^|\.)larksuite\.com$/.test(parsed.hostname)) {
      workspaceLink.href = parsed.href;
      workspaceLink.hidden = false;
    } else {
      workspaceLink.removeAttribute('href');
      workspaceLink.hidden = true;
    }
  } catch (_) {
    workspaceLink.removeAttribute('href');
    workspaceLink.hidden = true;
  }
  renderOnboarding();
}

function renderFeishuAuthorizationReady(authorized) {
  const button = document.getElementById('authorizeFeishuUserButton');
  const help = document.getElementById('feishuAuthorizationHelp');
  if (!button || !help) return;
  button.textContent = authorized ? '2 重新生成飞书授权链接' : '2 生成飞书授权链接';
  help.textContent = authorized
    ? '飞书账号已连接；如权限已撤回或过期，可重新授权。应用密钥和用户令牌保存在 Windows 凭据库。'
    : '先登录飞书并确认身份，再勾选上方声明、生成授权链接。之后可能依次确认应用创建与多维表格授权；请核对飞书实际权限清单。应用密钥和用户令牌保存在 Windows 凭据库。';
}

function renderFeishuAuthorizationProgress(progress) {
  const panel = document.getElementById('feishuAuthorizationPanel');
  const message = document.getElementById('feishuAuthorizationMessage');
  const link = document.getElementById('feishuAuthorizationLink');
  const code = document.getElementById('feishuAuthorizationCode');
  const button = document.getElementById('authorizeFeishuUserButton');
  const cancel = document.getElementById('cancelFeishuAuthorizationButton');
  if (!progress) return;
  const active = !progress.done && ['registering', 'authorizing', 'saving'].includes(progress.state);
  panel.hidden = false;
  message.textContent = progress.message || '飞书授权处理中。';
  message.classList.toggle('error', progress.state === 'denied' || progress.state === 'expired');
  try {
    const parsed = new URL(progress.verificationUrl || '');
    if (parsed.protocol === 'https:' && /(^|\.)feishu\.cn$|(^|\.)larksuite\.com$/.test(parsed.hostname)) {
      link.href = parsed.href;
      link.hidden = false;
    } else {
      link.removeAttribute('href');
      link.hidden = true;
    }
  } catch (_) {
    link.removeAttribute('href');
    link.hidden = true;
  }
  code.textContent = progress.userCode ? `飞书验证码：${progress.userCode}` : '';
  code.hidden = !progress.userCode;
  button.disabled = active;
  cancel.hidden = !active;
}

function isFeishuAuthorizationUrl(value) {
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && /(^|\.)feishu\.cn$|(^|\.)larksuite\.com$/.test(url.hostname);
  } catch (_) {
    return false;
  }
}

async function setAuthorizationWindowTopmost(enabled) {
  try {
    await appWindow?.setAlwaysOnTop(enabled);
  } catch (_) {
    // Opening the browser remains useful even if window layering cannot be changed.
  }
}

async function openFeishuIdentityPage() {
  if (typeof window.__TAURI__?.opener?.openUrl !== 'function') {
    showToast('无法调用系统浏览器，请重启应用后重试。');
    return;
  }
  const restoreTopmost = () => { void setAuthorizationWindowTopmost(true); };
  try {
    await setAuthorizationWindowTopmost(false);
    window.addEventListener('focus', restoreTopmost, { once: true });
    await window.__TAURI__.opener.openUrl('https://open.feishu.cn/app');
  } catch (_) {
    window.removeEventListener('focus', restoreTopmost);
    await setAuthorizationWindowTopmost(true);
    showToast('无法打开飞书页面，请检查默认浏览器设置后重试。');
  }
}

async function openFeishuAuthorizationPage(event) {
  event.preventDefault();
  const link = document.getElementById('feishuAuthorizationLink');
  const message = document.getElementById('feishuAuthorizationMessage');
  if (!link || !isFeishuAuthorizationUrl(link.href)) {
    message.textContent = '飞书授权链接无效，请重新发起授权。';
    message.classList.add('error');
    return;
  }
  if (typeof window.__TAURI__?.opener?.openUrl !== 'function') {
    message.textContent = '应用无法调用系统浏览器，请重启应用后重试。';
    message.classList.add('error');
    return;
  }
  message.textContent = '正在请求系统浏览器打开飞书授权页面…';
  message.classList.remove('error');
  try {
    await setAuthorizationWindowTopmost(false);
    await window.__TAURI__.opener.openUrl(link.href);
    message.textContent = '已在默认浏览器打开，请完成飞书页面确认。';
    message.classList.remove('error');
  } catch (error) {
    await setAuthorizationWindowTopmost(true);
    const reason = String(error).replace(/https?:\/\/\S+/g, '飞书授权链接');
    message.textContent = `无法打开飞书授权页面：${reason || '请检查默认浏览器设置后重试。'}`;
    message.classList.add('error');
  }
}

function scheduleFeishuAuthorizationPoll(seconds) {
  clearTimeout(feishuAuthPollTimer);
  feishuAuthPollTimer = setTimeout(() => pollFeishuUserAuthorization(), Math.max(1, Number(seconds) || 5) * 1000);
}

async function startFeishuUserAuthorization() {
  const accountConfirmation = document.getElementById('feishuAccountConfirmed');
  const accountWarning = document.getElementById('feishuAccountWarning');
  if (!accountConfirmation.checked) {
    accountWarning.textContent = '请先在飞书页面选择飞书身份（非豆包账号），再勾选确认。';
    accountWarning.hidden = false;
    accountConfirmation.focus();
    return;
  }
  // A user declaration is not an account-type detector or a permission check.
  accountConfirmation.checked = false;
  accountWarning.hidden = true;
  accountWarning.textContent = '';
  const button = document.getElementById('authorizeFeishuUserButton');
  button.disabled = true;
  button.textContent = '正在生成授权链接…';
  try {
    const progress = await call('start_feishu_user_authorization');
    renderFeishuAuthorizationProgress(progress);
    if (!progress.done) scheduleFeishuAuthorizationPoll(progress.retryAfterSeconds);
    else if (progress.state === 'authorized') await refreshFeishuProvisionStatus();
  } catch (error) {
    button.disabled = false;
    await refreshFeishuProvisionStatus().catch(() => {});
    showToast(`无法开始飞书授权：${String(error)}`);
  }
}

async function pollFeishuUserAuthorization() {
  try {
    const progress = await call('poll_feishu_user_authorization');
    renderFeishuAuthorizationProgress(progress);
    if (!progress.done) {
      scheduleFeishuAuthorizationPoll(progress.retryAfterSeconds);
      return;
    }
    clearTimeout(feishuAuthPollTimer);
    feishuAuthPollTimer = null;
    if (progress.state === 'authorized') {
      await refreshFeishuProvisionStatus();
      showToast('飞书账号授权完成');
    }
    await setAuthorizationWindowTopmost(true);
  } catch (error) {
    document.getElementById('feishuAuthorizationMessage').textContent = `暂时无法检查授权状态：${String(error)}。稍后会重试。`;
    scheduleFeishuAuthorizationPoll(8);
  }
}

async function stopWaitingForFeishuAuthorization() {
  clearTimeout(feishuAuthPollTimer);
  feishuAuthPollTimer = null;
  try {
    await call('cancel_feishu_user_authorization');
  } catch (error) {
    showToast(`停止等待失败：${String(error)}`);
  }
  document.getElementById('feishuAuthorizationPanel').hidden = true;
  await setAuthorizationWindowTopmost(true);
  const button = document.getElementById('authorizeFeishuUserButton');
  button.disabled = false;
  const status = await call('get_task_workspace_setup_status').catch(() => null);
  renderFeishuAuthorizationReady(status?.userAuthorized);
}

async function refreshFeishuProvisionStatus(result = null) {
  const [status, config, source] = await Promise.all([
    call('get_task_workspace_setup_status'),
    call('get_feishu_config'),
    call('get_active_task_source')
  ]);
  activeTaskSource = source;
  document.getElementById('activeSourceSelect').value = source;
  document.getElementById('activeSourceBadge').textContent = source === 'feishu' ? '飞书' : 'WPS';
  renderFeishuConnectionConfig(config);
  const workspaceUrl = result?.workspaceUrl || status?.workspaceUrl;
  if (workspaceUrl) document.getElementById('feishuTableUrlInput').value = workspaceUrl;
  renderFeishuProvisionStatus(status, config, source, result);
  return status;
}

async function completeFeishuWorkspaceSetup(successMessage) {
  workspaceFirstSyncFailed = !(await syncFromSource(true));
  if (!workspaceFirstSyncFailed) {
    showToast(successMessage);
    return;
  }
  const resultText = document.getElementById('feishuProvisionResult');
  resultText.hidden = false;
  resultText.textContent = '任务空间已创建并连接，但首次同步未完成。连接和副本链接已保留，请点击顶部同步重试，不要重新创建。';
  showToast('任务空间已连接，首次同步未完成；请重试同步，不要重新创建');
}

async function createFeishuWorkspace() {
  if (workspaceSetupBusy || document.getElementById('createFeishuWorkspaceButton').disabled) return;
  const button = document.getElementById('createFeishuWorkspaceButton');
  const workspaceName = document.getElementById('feishuWorkspaceNameInput').value.trim();
  const folderLink = document.getElementById('feishuWorkspaceFolderLinkInput').value.trim();
  if (!workspaceName || workspaceName.length > 100) {
    const error = document.getElementById('feishuWorkspaceNameError');
    error.hidden = false;
    error.textContent = '请填写 1–100 个字符的任务空间名称。';
    document.getElementById('feishuWorkspaceNameInput').setAttribute('aria-invalid', 'true');
    document.getElementById('feishuWorkspaceNameInput').focus();
    showToast('请填写不超过 100 个字符的任务空间名称');
    return;
  }
  button.disabled = true;
  document.getElementById('feishuWorkspaceNameError').hidden = true;
  document.getElementById('feishuWorkspaceNameInput').removeAttribute('aria-invalid');
  workspaceSetupBusy = true;
  workspaceFirstSyncFailed = false;
  renderOnboarding();
  button.textContent = '正在创建…';
  try {
    const result = await call('start_feishu_task_workspace_setup', {
      workspaceName,
      folderLink: folderLink || null
    });
    await refreshFeishuProvisionStatus(result);
    if (result.state === 'completed') {
      await completeFeishuWorkspaceSetup('飞书空间已创建并连接，云端任务已读入浮窗；本地工作台尚未同步');
    } else {
      showToast(result.message || `飞书任务空间状态：${result.state}`);
    }
  } catch (error) {
    await refreshFeishuProvisionStatus().catch(() => {});
    document.getElementById('feishuProvisionResult').hidden = false;
    document.getElementById('feishuProvisionResult').textContent = `未能开始初始化：${String(error)}`;
    showToast(`创建任务空间失败：${String(error)}`);
  } finally {
    workspaceSetupBusy = false;
    renderOnboarding();
    if (!onboardingView.hidden && onboardingSetup?.config?.appToken) onboardingView.scrollTop = 0;
    button.textContent = '创建并连接';
  }
}

async function resumeFeishuWorkspace() {
  if (workspaceSetupBusy) return;
  workspaceSetupBusy = true;
  renderOnboarding();
  const button = document.getElementById('resumeFeishuWorkspaceButton');
  button.disabled = true;
  button.textContent = '继续中…';
  try {
    const result = await call('resume_feishu_task_workspace_setup');
    await refreshFeishuProvisionStatus(result);
    if (result?.state === 'completed') {
      await completeFeishuWorkspaceSetup('飞书空间已连接，云端任务已读入浮窗；本地工作台尚未同步');
    } else if (result) {
      showToast(result.message || `初始化状态：${result.state}`);
    } else {
      showToast('没有可继续的初始化记录');
    }
  } catch (error) {
    await refreshFeishuProvisionStatus().catch(() => {});
    document.getElementById('feishuProvisionResult').hidden = false;
    document.getElementById('feishuProvisionResult').textContent = `恢复未完成：${String(error)}`;
    showToast(`继续初始化失败：${String(error)}`);
  } finally {
    workspaceSetupBusy = false;
    renderOnboarding();
    if (!onboardingView.hidden && onboardingSetup?.config?.appToken) onboardingView.scrollTop = 0;
    button.textContent = '继续上次初始化';
  }
}

async function reconcileFeishuWorkspace() {
  const found = window.confirm('请先检查飞书中的任务空间：是否已经看到本次计划创建的副本？\n\n选择“确定”表示已找到副本；选择“取消”表示确认没有创建成功。');
  let appToken = null;
  let workspaceUrl = null;
  if (found) {
    const supplied = window.prompt('粘贴副本的飞书多维表格链接，或只填写链接中 base/ 后的 App Token。');
    if (!supplied?.trim()) return;
    const value = supplied.trim();
    try {
      const parsed = new URL(value);
      const parts = parsed.pathname.split('/').filter(Boolean);
      const baseIndex = parts.indexOf('base');
      appToken = baseIndex >= 0 ? parts[baseIndex + 1] : null;
      if (!appToken || parsed.protocol !== 'https:' || !/(^|\.)feishu\.cn$|(^|\.)larksuite\.com$/.test(parsed.hostname)) {
        showToast('链接格式不正确，请粘贴飞书 base 链接或仅填写 App Token');
        return;
      }
      workspaceUrl = parsed.href;
    } catch (_) {
      appToken = value;
    }
    if (!/^[A-Za-z0-9]+$/.test(appToken || '')) {
      showToast('App Token 格式不正确');
      return;
    }
  }
  const button = document.getElementById('reconcileFeishuWorkspaceButton');
  button.disabled = true;
  try {
    await call('reconcile_feishu_task_workspace_setup', {
      remoteWorkspaceExists: found,
      appToken,
      workspaceUrl
    });
    await refreshFeishuProvisionStatus();
    showToast(found ? '已保存副本标识，可继续自动发现任务表' : '已确认没有创建副本；现在可以安全重新开始');
  } catch (error) {
    showToast(`核对失败：${String(error)}`);
  } finally {
    button.disabled = false;
  }
}

function queueStateLabel(state) {
  if (state === 'pending') return '待同步';
  if (state === 'conflict') return '发生冲突';
  if (state === 'uncertain') return '结果待核对';
  if (state === 'editing') return '重新分析中';
  return state;
}

function renderQueueItems(items) {
  document.getElementById('queueCountBadge').textContent = `${items.length} 项`;
  const list = document.getElementById('queueList');
  list.innerHTML = items.length ? items.map(item => {
    const input = item.input;
    const title = input.temporaryTitle || input.description;
    const retryLabel = item.state === 'uncertain' ? '核对后重分析' : item.state === 'editing' ? '继续分析' : '重新分析';
    return `<article class="queue-item" data-state="${escapeHtml(item.state)}"><strong>${escapeHtml(title)}</strong><span class="queue-state">${escapeHtml(queueStateLabel(item.state))}</span><p>${escapeHtml(input.description)} · ${input.progress}%${item.lastError ? ` · ${escapeHtml(item.lastError)}` : ''}</p><div class="queue-actions"><button type="button" data-queue-action="reanalyze" data-queue-id="${item.id}">${retryLabel}</button><button class="danger" type="button" data-queue-action="discard" data-queue-id="${item.id}">放弃</button></div></article>`;
  }).join('') : '<div class="privacy-note"><strong>没有待处理记录</strong><p>离线记录会在恢复网络后自动同步；只有冲突或结果不确定时才需要人工处理。</p></div>';
}

async function refreshQueueItems() {
  const items = await call('list_pending_write_items');
  renderQueueItems(items);
  await refreshPendingWriteSummary();
}

function renderHabits(habits) {
  document.getElementById('habitCountBadge').textContent = `${habits.length} 条`;
  const list = document.getElementById('habitList');
  list.innerHTML = habits.length ? habits.map(habit => `<article class="habit-item"><strong>${escapeHtml(habit.phrase)} → ${escapeHtml(habit.interpretation)}</strong><p>${habit.kind === 'task_type' ? `固定任务类型 · ${escapeHtml(labels[habit.quadrant] || '未分类')}` : habit.kind === 'correction' ? '纠正记录' : '已有任务别名'} · 已命中 ${habit.hitCount} 次</p><button class="habit-delete" type="button" data-habit-id="${habit.id}" aria-label="删除这条习惯">删除</button></article>`).join('') : '<div class="privacy-note"><strong>还没有习惯记录</strong><p>在任务匹配确认卡中更正或勾选“记住这个说法”后，会显示在这里。</p></div>';
}

async function refreshHabits() {
  renderHabits(await call('list_habits'));
}

function updateAiProviderUi(clearModels = true) {
  const custom = document.getElementById('aiProviderSelect').value === 'generic';
  document.getElementById('aiEndpointField').hidden = !custom;
  if (clearModels) {
    const select = document.getElementById('aiModelSelect');
    select.innerHTML = '<option value="">请先读取模型</option>';
    select.disabled = true;
    document.getElementById('aiConfigBadge').textContent = '待读取';
  }
}

async function loadAiModels() {
  const button = document.getElementById('loadAiModelsButton');
  const provider = document.getElementById('aiProviderSelect').value;
  const endpoint = document.getElementById('aiEndpointInput').value.trim();
  if (provider === 'generic' && !endpoint) {
    showToast('请先填写自定义接口地址');
    return;
  }
  button.disabled = true;
  button.textContent = '正在读取…';
  try {
    const result = await call('list_ai_models', {
      input: {
        provider,
        endpoint,
        apiKey: document.getElementById('aiKeyInput').value.trim() || null
      }
    });
    const select = document.getElementById('aiModelSelect');
    select.innerHTML = result.models.map(model => `<option value="${escapeHtml(model)}">${escapeHtml(model)}</option>`).join('');
    select.disabled = false;
    document.getElementById('aiConfigBadge').textContent = `已读取 ${result.models.length} 个`;
    document.getElementById('aiConfigHelp').textContent = '请在模型下拉菜单中选择用于任务匹配的模型。';
    showToast(`已读取 ${result.models.length} 个可用模型`);
  } catch (error) {
    document.getElementById('aiConfigBadge').textContent = '读取失败';
    document.getElementById('aiConfigHelp').textContent = String(error);
    showToast(`读取模型失败：${String(error)}`);
  } finally {
    button.disabled = false;
    button.textContent = '读取可用模型';
  }
}

async function saveAiSettings(event) {
  event.preventDefault();
  const button = document.getElementById('saveAiButton');
  button.disabled = true;
  try {
    const config = await call('save_ai_config', {
      input: {
        provider: document.getElementById('aiProviderSelect').value,
        endpoint: document.getElementById('aiEndpointInput').value.trim(),
        model: document.getElementById('aiModelSelect').value,
        apiKey: document.getElementById('aiKeyInput').value.trim() || null,
        enabled: document.getElementById('aiEnabledToggle').getAttribute('aria-checked') === 'true'
      }
    });
    document.getElementById('aiKeyInput').value = '';
    document.getElementById('aiKeyInput').placeholder = '密钥已安全保存；留空表示不修改';
    document.getElementById('aiConfigBadge').textContent = config.apiKeyConfigured ? (config.enabled ? '已启用' : '已停用') : '缺少密钥';
    document.getElementById('aiConfigHelp').textContent = '模型设置已保存；精确任务名仍由本地规则处理。';
    showToast('AI 模型设置已保存到本机');
  } catch (error) {
    showToast(`保存失败：${String(error)}`);
  } finally {
    button.disabled = false;
  }
}

async function testAiConnection() {
  const button = document.getElementById('testAiButton');
  button.disabled = true;
  button.textContent = '测试中…';
  try {
    await call('test_ai_connection');
    document.getElementById('aiConfigBadge').textContent = '连接正常';
    document.getElementById('aiConfigHelp').textContent = '模型接口已返回有效内容。测试会产生一次极小调用。';
    showToast('AI 模型连接测试成功');
  } catch (error) {
    document.getElementById('aiConfigBadge').textContent = '连接失败';
    document.getElementById('aiConfigHelp').textContent = String(error);
    showToast(`模型测试失败：${String(error)}`);
  } finally {
    button.disabled = false;
    button.textContent = '测试模型';
  }
}

async function saveWpsSettings(event) {
  event.preventDefault();
  const button = document.getElementById('saveWpsButton');
  button.disabled = true;
  try {
    const config = await call('save_wps_config', {
      input: {
        webhookUrl: document.getElementById('wpsWebhookInput').value.trim(),
        sheetName: document.getElementById('wpsSheetInput').value.trim(),
        token: document.getElementById('wpsTokenInput').value.trim() || null
      }
    });
    document.getElementById('wpsTokenInput').value = '';
    document.getElementById('wpsTokenInput').placeholder = '令牌已安全保存；留空表示不修改';
    document.getElementById('wpsConfigBadge').textContent = config.tokenConfigured ? '已保存' : '缺少令牌';
    document.getElementById('wpsConfigHelp').textContent = '连接信息已保存；测试读取不会修改线上表格。';
    showToast('WPS 连接已保存到本机');
  } catch (error) {
    showToast(`保存失败：${String(error)}`);
  } finally {
    button.disabled = false;
  }
}

async function testWpsConnection() {
  const button = document.getElementById('testWpsButton');
  button.disabled = true;
  button.textContent = '读取中…';
  try {
    const result = await call('test_wps_connection');
    document.getElementById('wpsSheetInput').value = result.sheetName;
    document.getElementById('wpsConfigBadge').textContent = result.writeReady ? '读写就绪' : '仅可读取';
    document.getElementById('wpsConfigHelp').textContent = result.writeReady
      ? result.undoReady
        ? `已识别「${result.sheetName}」：${result.taskCount} 条记录，写入和云端撤销均已就绪。`
        : `已识别「${result.sheetName}」：${result.taskCount} 条记录；请更新 AirScript v17 后再写入或撤销。`
      : `已识别 ${result.taskCount} 条记录；请更新项目提供的 AirScript 后再启用写入。`;
    showToast(result.writeReady ? 'WPS 读写接口已就绪' : '读取正常，但 AirScript 需更新');
  } catch (error) {
    document.getElementById('wpsConfigBadge').textContent = '读取失败';
    document.getElementById('wpsConfigHelp').textContent = String(error);
    showToast(`测试失败：${String(error)}`);
  } finally {
    button.disabled = false;
    button.textContent = '测试读取';
  }
}

async function saveFeishuSettings(event) {
  event.preventDefault();
  const button = document.getElementById('saveFeishuButton');
  button.disabled = true;
  try {
    const config = await call('save_feishu_config', {
      input: {
        appId: document.getElementById('feishuAppIdInput').value.trim(),
        appSecret: document.getElementById('feishuAppSecretInput').value.trim() || null,
        appToken: document.getElementById('feishuAppTokenInput').value.trim(),
        tableId: document.getElementById('feishuTableIdInput').value.trim()
      }
    });
    document.getElementById('feishuAppSecretInput').value = '';
    document.getElementById('feishuAppSecretInput').placeholder = '密钥已安全保存；留空表示不修改';
    document.getElementById('feishuConfigBadge').textContent = config.secretConfigured ? '已保存' : '缺少密钥';
    document.getElementById('feishuConfigHelp').textContent = '连接信息已保存；测试读取不会修改飞书表格。';
    showToast('飞书连接已保存到本机');
  } catch (error) {
    showToast(`保存失败：${String(error)}`);
  } finally {
    button.disabled = false;
  }
}

function parseFeishuTableUrl() {
  const value = document.getElementById('feishuTableUrlInput').value.trim();
  if (!value) return;
  try {
    const url = new URL(value);
    const parts = url.pathname.split('/').filter(Boolean);
    const baseIndex = parts.indexOf('base');
    const appToken = baseIndex >= 0 ? parts[baseIndex + 1] : '';
    const tableId = url.searchParams.get('table') || '';
    if (!appToken || !tableId) {
      showToast('请粘贴直接打开多维表格的 base 链接，并确认链接包含 table=tbl...');
      return;
    }
    document.getElementById('feishuAppTokenInput').value = appToken;
    document.getElementById('feishuTableIdInput').value = tableId;
    document.getElementById('feishuConfigHelp').textContent = '已从链接提取 App Token 和数据表 ID。';
  } catch (_) {
    showToast('飞书多维表格链接格式不正确');
  }
}

async function testFeishuConnection() {
  const button = document.getElementById('testFeishuButton');
  button.disabled = true;
  button.textContent = '读取中…';
  try {
    const result = await call('test_feishu_connection');
    document.getElementById('feishuConfigBadge').textContent = formalFloatingWrites ? '可确认提交' : result.writeReady ? '隔离验收' : '读取就绪';
    document.getElementById('feishuConfigHelp').textContent = formalFloatingWrites
      ? `已识别 ${result.taskCount} 条记录；浮窗提交前会核对授权账号和目标表，并弹出本次操作确认。读取成功不代表飞书已授予写权限，仍需表格编辑权限；不会自动写入。`
      : result.writeReady
      ? `已识别 ${result.taskCount} 条记录和 ${result.fields.length} 个字段；仅指定测试表允许写入与撤销，正式版仍保持只读。`
      : `已识别 ${result.taskCount} 条记录和 ${result.fields.length} 个字段；新增、更新与撤销逻辑待当前连接完成真实写入验收后启用。`;
    showToast(`飞书读取正常：${result.taskCount} 项任务`);
  } catch (error) {
    const message = String(error);
    document.getElementById('feishuConfigBadge').textContent = '读取失败';
    document.getElementById('feishuConfigHelp').textContent = message;
    if (message.includes('缺少必需字段')) {
      document.getElementById('feishuSchemaHelp').textContent = '可点击“检查任务空间”查看字段、看板、仪表盘和自动化；确认前不会修改飞书表格。';
    }
    showToast(`测试失败：${message}`);
  } finally {
    button.disabled = false;
    button.textContent = '测试读取';
  }
}

async function downloadFeishuTemplate() {
  const button = document.getElementById('downloadFeishuTemplateButton');
  button.disabled = true;
  button.textContent = '选择保存位置…';
  try {
    const result = await call('save_feishu_template');
    if (!result.saved) {
      showToast('已取消保存模板');
      return;
    }
    document.getElementById('feishuSchemaHelp').textContent = `已保存 ${result.fileName || '飞书字段导入模板.xlsx'}；该文件只包含字段，不包含飞书原生看板、仪表盘或自动化。`;
    showToast('备用字段模板已保存');
  } catch (error) {
    document.getElementById('feishuSchemaHelp').textContent = String(error);
    showToast(`模板保存失败：${String(error)}`);
  } finally {
    button.disabled = false;
    button.textContent = '下载字段模板（备用）';
  }
}

function workspaceSummary(status) {
  const fields = status.missingRequiredFields?.length
    ? `缺少字段：${status.missingRequiredFields.join('、')}`
    : '任务字段：完整';
  const kanban = status.hasKanban ? '任务看板：已有' : '任务看板：缺少';
  const dashboards = `仪表盘：${status.dashboards?.length || 0} 个`;
  const workflowCheckLimited = (status.inspectionWarnings || []).some(warning => warning.includes('读取自动化'));
  const workflows = workflowCheckLimited
    ? '自动化：权限受限，未能核实'
    : `自动化：${status.workflows?.length || 0} 个`;
  const warnings = status.inspectionWarnings?.length
    ? `；部分检查受限：${status.inspectionWarnings.join('；')}`
    : '';
  return `${fields}；${kanban}；${dashboards}；${workflows}${warnings}`;
}

function isFeishuWorkspaceReady(status) {
  const warnings = status?.inspectionWarnings || [];
  const automationReady = !warnings.some(warning =>
    warning.includes('读取自动化') || warning.includes('状态联动自动化未就绪')
  );
  return Boolean(!status?.missingRequiredFields?.length && status?.hasKanban && (status?.dashboards?.length || 0) && automationReady);
}

async function initializeFeishuWorkspace() {
  const button = document.getElementById('initializeFeishuWorkspaceButton');
  button.disabled = true;
  button.textContent = '检查中…';
  try {
    const plan = await call('inspect_feishu_workspace');
    const missing = plan.missingRequiredFields || [];
    const canAssessViews = !(plan.inspectionWarnings || []).some(warning => warning.includes('读取视图'));
    const needsKanban = canAssessViews && !plan.hasKanban;
    const changes = [
      ...missing.map(name => `新增字段“${name}”`),
      ...(needsKanban ? ['新增看板视图“任务看板”'] : [])
    ];

    if (!changes.length) {
      const workspaceReady = isFeishuWorkspaceReady(plan);
      document.getElementById('feishuConfigBadge').textContent = workspaceReady ? '任务空间就绪' : '需完善模板';
      document.getElementById('feishuSchemaHelp').textContent = workspaceSummary(plan);
      showToast(workspaceReady ? '完整任务空间检查通过' : '检查完成：请查看缺少或未启用的工具');
      return;
    }

    const [appId, spaceId, tableId] = JSON.parse(plan.targetKey);
    if (![appId, spaceId, tableId].every(v => typeof v === 'string' && v) || !Number.isSafeInteger(plan.contextVersion)) throw new Error('目标检查信息不完整，请重新检查');
    const confirmed = window.confirm(
      `任务空间检查结果：\n\n${workspaceSummary(plan)}\n\n应用：${appId}\n目标空间：${spaceId}\n目标表：${tableId}\n将执行：\n${changes.map(item => `• ${item}`).join('\n')}\n\n计划完成日期和开始日期允许留空，新日期列不会自动填入日期。不会覆盖或删除现有字段、记录、视图、仪表盘和自动化。是否继续？`
    );
    if (!confirmed) {
      document.getElementById('feishuSchemaHelp').textContent = `已取消修改。${workspaceSummary(plan)}`;
      showToast('未修改飞书任务空间');
      return;
    }

    button.textContent = '补齐中…';
    const result = await call('initialize_feishu_workspace', { expectedTargetKey: plan.targetKey, contextVersion: plan.contextVersion });
    const status = result.status;
    document.getElementById('feishuConfigBadge').textContent = isFeishuWorkspaceReady(status) ? '任务空间就绪' : '需完善模板';
    document.getElementById('feishuSchemaHelp').textContent = `${workspaceSummary(status)}。本次新增字段 ${result.createdFields?.length || 0} 个、看板 ${result.createdViews?.length || 0} 个。`;
    showToast('飞书任务空间已检查并补齐');
  } catch (error) {
    document.getElementById('feishuConfigBadge').textContent = '检查失败';
    document.getElementById('feishuSchemaHelp').textContent = String(error);
    showToast(`任务空间检查失败：${String(error)}`);
  } finally {
    button.disabled = false;
    button.textContent = '检查任务空间';
  }
}

async function switchActiveSource() {
  const button = document.getElementById('saveActiveSourceButton');
  const selected = document.getElementById('activeSourceSelect').value;
  const previous = activeTaskSource;
  button.disabled = true;
  try {
    clearTimeout(toastTimer); toast.hidden = true; toastPriority = 'normal'; hideAnalysis();
    // Finish the previous source's read before changing the target of the next sync.
    if (syncInFlight) await syncInFlight;
    await call('set_active_task_source', { source: selected });
    activeTaskSource = selected;
    if (!(await syncFromSource(false))) {
      await call('set_active_task_source', { source: previous });
      activeTaskSource = previous;
      document.getElementById('activeSourceSelect').value = previous;
      document.getElementById('activeSourceBadge').textContent = previous === 'feishu' ? '飞书' : 'WPS';
      showToast('数据源未切换：请先完成所选数据源的连接测试');
      return;
    }
    document.getElementById('activeSourceBadge').textContent = selected === 'feishu' ? '飞书' : 'WPS';
    showToast(`当前任务数据源已切换为${selected === 'feishu' ? '飞书' : 'WPS'}`);
  } catch (error) {
    showToast(`切换失败：${String(error)}`);
  } finally {
    button.disabled = false;
  }
}

function showRecordView() {
  onboardingView.hidden = true;
  settingsView.hidden = true;
  recordView.hidden = false;
}

document.getElementById('expandButton').addEventListener('click', () => expand().catch(error => showToast(String(error))));
document.getElementById('collapseButton').addEventListener('click', () => collapse().catch(error => showToast(String(error))));
document.getElementById('progressForm').addEventListener('submit', analyze);
workInput.addEventListener('input', () => { hideAnalysis(); validate(); });
progressInput.addEventListener('input', () => { hideAnalysis(); validate(); });
document.getElementById('settingsButton').addEventListener('click', showSettings);
document.getElementById('closeSettings').addEventListener('click', showRecordView);
document.getElementById('openOnboardingButton').addEventListener('click', () => showOnboarding('feishu'));
document.getElementById('onboardingFeishuButton').addEventListener('click', () => {
  onboardingPage = 'feishu'; renderOnboarding();
  const target = onboardingSetup?.status?.userAuthorized ? 'feishuWorkspaceNameInput' : 'openFeishuIdentityButton';
  if (!document.getElementById(target).hidden && !onboardingSetup?.config?.appToken) document.getElementById(target).focus();
});
document.getElementById('onboardingWpsButton').addEventListener('click', () => openAdvancedConnection('wps'));
document.getElementById('onboardingExistingButton').addEventListener('click', () => openAdvancedConnection());
document.getElementById('onboardingRepairConnectionButton').addEventListener('click', () => openAdvancedConnection());
document.getElementById('onboardingBackButton').addEventListener('click', leaveOnboarding);
async function openLocalWorkbench() {
  try { await invoke('open_local_workbench'); onboardingDeferred = true; }
  catch (error) { showToast(`打开本地工作台失败：${error}`); }
}
document.getElementById('onboardingLocalButton').addEventListener('click', openLocalWorkbench);
document.getElementById('openLocalWorkbenchButton').addEventListener('click', openLocalWorkbench);
document.getElementById('recordWorkbenchButton').addEventListener('click', openLocalWorkbench);
document.getElementById('onboardingFinishButton').addEventListener('click', openLocalWorkbench);
document.getElementById('onboardingRetrySyncButton').addEventListener('click', async () => {
  const button = document.getElementById('onboardingRetrySyncButton');
  button.disabled = true;
  try { await completeFeishuWorkspaceSetup('云端任务已读入浮窗；本地工作台同步需另行确认'); renderOnboarding(); }
  finally { button.disabled = false; }
});
document.getElementById('feishuWorkspaceLink').addEventListener('click', async event => {
  event.preventDefault();
  const url = event.currentTarget.href;
  if (!isFeishuAuthorizationUrl(url)) return;
  try { await setAuthorizationWindowTopmost(false); await window.__TAURI__.opener.openUrl(url); }
  catch (error) { await setAuthorizationWindowTopmost(true); showToast(`无法打开任务空间：${String(error)}`); }
});
document.getElementById('syncButton').addEventListener('click', () => syncFromSource(false));
document.getElementById('wpsConfigForm').addEventListener('submit', saveWpsSettings);
document.getElementById('testWpsButton').addEventListener('click', testWpsConnection);
document.getElementById('feishuConfigForm').addEventListener('submit', saveFeishuSettings);
document.getElementById('openFeishuIdentityButton').addEventListener('click', openFeishuIdentityPage);
document.getElementById('authorizeFeishuUserButton').addEventListener('click', startFeishuUserAuthorization);
document.getElementById('feishuAuthorizationLink').addEventListener('click', openFeishuAuthorizationPage);
document.getElementById('cancelFeishuAuthorizationButton').addEventListener('click', stopWaitingForFeishuAuthorization);
document.getElementById('createFeishuWorkspaceButton').addEventListener('click', createFeishuWorkspace);
document.getElementById('resumeFeishuWorkspaceButton').addEventListener('click', resumeFeishuWorkspace);
document.getElementById('reconcileFeishuWorkspaceButton').addEventListener('click', reconcileFeishuWorkspace);
document.getElementById('testFeishuButton').addEventListener('click', testFeishuConnection);
document.getElementById('downloadFeishuTemplateButton').addEventListener('click', downloadFeishuTemplate);
document.getElementById('initializeFeishuWorkspaceButton').addEventListener('click', initializeFeishuWorkspace);
document.getElementById('feishuTableUrlInput').addEventListener('change', parseFeishuTableUrl);
document.getElementById('saveActiveSourceButton').addEventListener('click', switchActiveSource);
document.getElementById('aiConfigForm').addEventListener('submit', saveAiSettings);
document.getElementById('loadAiModelsButton').addEventListener('click', loadAiModels);
document.getElementById('testAiButton').addEventListener('click', testAiConnection);
document.getElementById('aiProviderSelect').addEventListener('change', () => updateAiProviderUi(true));
document.getElementById('aiEnabledToggle').addEventListener('click', event => {
  const enabled = event.currentTarget.getAttribute('aria-checked') !== 'true';
  event.currentTarget.setAttribute('aria-checked', String(enabled));
});
function startWindowDrag(event) {
  if (event.button !== 0 || event.target.closest('button')) return;
  appWindow?.startDragging().catch(error => showToast(String(error)));
}
document.getElementById('collapsedDragRegion').addEventListener('mousedown', startWindowDrag);
document.getElementById('collapsedProgressRegion').addEventListener('mousedown', startWindowDrag);
document.getElementById('dragHandle').addEventListener('mousedown', startWindowDrag);
document.addEventListener('contextmenu', event => {
  event.preventDefault();
  call('hide_window').catch(error => showToast(String(error)));
});

analysisResult.addEventListener('click', event => {
  const candidate = event.target.closest('[data-task-id]');
  const quadrant = event.target.closest('[data-quadrant]');
  const action = event.target.closest('[data-action]')?.dataset.action;
  if (candidate) {
    if (candidate.dataset.taskId === 'temporary') {
      correctionMode = true;
      renderTemporary({
        suggested_title: workInput.value.trim().slice(0, 24),
        suggested_detail: workInput.value.trim(),
        suggested_quadrant: null,
        suggested_phrase: null,
        next_progress: Number(progressInput.value),
        analysis_source: 'local',
        confidence: 0
      });
      return;
    }
    analysisResult.querySelectorAll('[data-task-id]').forEach(button => button.setAttribute('aria-pressed', 'false'));
    candidate.setAttribute('aria-pressed', 'true');
    selectedTaskId = candidate.dataset.taskId;
    selectedTaskSnapshot = {
      title: candidate.dataset.title || '',
      progress: Number(candidate.dataset.progress),
      status: candidate.dataset.status || '',
      plannedDate: candidate.dataset.plannedDate || null
    };
    temporaryTitle = null;
    selectedQuadrant = null;
    analysisResult.querySelector('[data-action="confirm"]').disabled = plannedDateChangeRequested && !selectedPlannedDate;
  }
  if (quadrant) {
    analysisResult.querySelectorAll('[data-quadrant]').forEach(button => button.setAttribute('aria-pressed', 'false'));
    quadrant.setAttribute('aria-pressed', 'true');
    selectedQuadrant = quadrant.dataset.quadrant;
    const currentTitle = document.getElementById('temporaryTitleInput')?.value.trim();
    analysisResult.querySelector('[data-action="confirm"]').disabled = document.getElementById('temporaryTitleInput') ? !currentTitle : false;
  }
  if (action === 'cancel') {
    hideAnalysis();
    workInput.focus();
  }
  if (action === 'correct') showCorrectionPicker().catch(error => showToast(`无法读取任务：${String(error)}`));
  if (action === 'confirm') confirm();
});

analysisResult.addEventListener('input', event => {
  if (event.target.id === 'temporaryDetailInput') {
    temporaryDetail = event.target.value.trim();
    return;
  }
  if (event.target.id === 'plannedDateInput') {
    selectedPlannedDate = event.target.value || null;
    const confirmButton = analysisResult.querySelector('[data-action="confirm"]');
    if (confirmButton && selectedTaskId !== 'temporary') {
      confirmButton.disabled = !selectedTaskId || (plannedDateChangeRequested && !selectedPlannedDate);
    }
    return;
  }
  if (event.target.id === 'habitPhraseInput') {
    const hint = document.getElementById('taskTypeLearningHint');
    if (hint) hint.textContent = `以后出现“${event.target.value.trim()}”时，默认新增为“${temporaryTitle || ''}”。`;
    return;
  }
  if (event.target.id !== 'temporaryTitleInput') return;
  temporaryTitle = event.target.value.trim();
  const checkbox = document.getElementById('rememberHabitInput');
  const phraseInput = document.getElementById('habitPhraseInput');
  if (checkbox && phraseInput) {
    checkbox.checked = true;
    phraseInput.value = suggestTaskTypePhrase(workInput.value, temporaryTitle);
    const hint = document.getElementById('taskTypeLearningHint');
    if (hint) hint.textContent = `以后出现“${phraseInput.value}”时，默认新增为“${temporaryTitle}”。`;
  }
  const confirmButton = analysisResult.querySelector('[data-action="confirm"]');
  if (confirmButton) confirmButton.disabled = !temporaryTitle || !selectedQuadrant;
});

document.getElementById('toggleHabitsButton').addEventListener('click', event => {
  const panel = document.getElementById('habitListPanel');
  const expanded = panel.hidden;
  panel.hidden = !expanded;
  event.currentTarget.setAttribute('aria-expanded', String(expanded));
  event.currentTarget.textContent = expanded ? '收起习惯' : '查看习惯';
});

document.getElementById('toggleQueueButton').addEventListener('click', event => {
  const panel = document.getElementById('queueListPanel');
  const expanded = panel.hidden;
  panel.hidden = !expanded;
  event.currentTarget.setAttribute('aria-expanded', String(expanded));
  event.currentTarget.textContent = expanded ? '收起队列' : '查看队列';
});

document.getElementById('queueList').addEventListener('click', async event => {
  const button = event.target.closest('[data-queue-action]');
  if (!button) return;
  const id = Number(button.dataset.queueId);
  button.disabled = true;
  try {
    if (button.dataset.queueAction === 'discard') {
      if (!window.confirm('放弃后这条记录不会写入 WPS，确定继续吗？')) {
        button.disabled = false;
        return;
      }
      await call('discard_pending_write', { id });
      if (editingQueueId === id) editingQueueId = null;
      await refreshQueueItems();
      showToast('已放弃这条待同步记录');
      return;
    }
    const input = await call('prepare_pending_write', { id });
    editingQueueId = id;
    showRecordView();
    hideAnalysis();
    workInput.value = input.description;
    progressInput.value = input.progress;
    validate();
    document.getElementById('progressForm').requestSubmit(input.taskId ? updateTaskButton : createTaskButton);
  } catch (error) {
    button.disabled = false;
    showToast(`队列操作失败：${String(error)}`);
  }
});

document.getElementById('habitList').addEventListener('click', async event => {
  const button = event.target.closest('[data-habit-id]');
  if (!button) return;
  button.disabled = true;
  try {
    await call('delete_habit', { id: Number(button.dataset.habitId) });
    await refreshHabits();
    showToast('已删除这条习惯');
  } catch (error) {
    button.disabled = false;
    showToast(`删除失败：${String(error)}`);
  }
});

document.getElementById('autostartToggle').addEventListener('click', async event => {
  const button = event.currentTarget;
  const enabled = button.getAttribute('aria-checked') !== 'true';
  try {
    await call('set_autostart', { enabled });
    button.setAttribute('aria-checked', String(enabled));
    showToast(enabled ? '已开启随 Windows 启动' : '已关闭随 Windows 启动');
  } catch (error) {
    showToast(`设置失败：${String(error)}`);
  }
});

document.addEventListener('keydown', event => {
  if (document.getElementById('taskRemovalDialog').open) {
    if (event.key === 'Escape') { event.preventDefault(); closeTaskRemoval(); }
    return;
  }
  if (event.key === 'Escape') {
    if (!onboardingView.hidden) leaveOnboarding();
    else if (!settingsView.hidden) showRecordView();
    else if (!analysisResult.hidden) hideAnalysis();
    else collapse().catch(error => showToast(String(error)));
  }
});

if (listen) {
  listen('feishu-connection-changed', () => { clearTimeout(toastTimer); toast.hidden = true; toastPriority = 'normal'; hideAnalysis(); floatingTaskContext = null; if (!taskRemovalBusy) closeTaskRemoval(); });
  listen('open-feishu-setup', async () => { if (app.classList.contains('collapsed')) await expand(); await showOnboarding('feishu'); });
  listen('open-settings', async () => { if (app.classList.contains('collapsed')) await expand(); await showSettings(); });
  listen('local-workbench-open-failed', event => showToast(`打开本地工作台失败：${String(event.payload)}`));
  listen('request-sync', async () => { if (app.classList.contains('collapsed')) await expand(); await syncFromSource(false); });
  listen('task-sync-complete', async event => {
    await refreshTasks();
    const label = event.payload.source === 'feishu' ? '飞书' : 'WPS';
    await refreshPendingWriteSummary(syncedTaskStatus(label, event.payload));
  });
  listen('pending-writes-changed', event => applyPendingWriteSummary(event.payload));
  listen('task-sync-failed', async event => {
    try {
      const summary = await call('get_pending_write_summary');
      if (!applyPendingWriteSummary(summary)) setSyncState('offline', '离线，尚未同步');
    } catch (_) {
      setSyncState('offline', '离线，尚未同步');
    }
    console.warn('WPS background sync failed:', event.payload);
  });
}

// Reuse the single set of authorization/provisioning controls in the dedicated wizard.
document.getElementById('feishuProvisionHost').appendChild(document.getElementById('feishuProvisionCard'));
refreshTasks().catch(() => {});
refreshPendingWriteSummary().catch(() => {});
call('get_write_acceptance_status').then(status => {
  formalFloatingWrites = status.floatingWriteEnabled === true;
  if (!status.enabled) return;
  document.querySelector('.header-copy h1').textContent = '隔离写入验收';
  document.title = '个人任务进度隔离写入验收';
  document.getElementById('feishuConfigHelp').textContent = '仅允许指定验收空间的任务表写入与撤销；不开放正式版写入。';
  document.getElementById('onboardingWriteNotice').textContent = '隔离验收版：仅指定空白验收空间允许写入与撤销，正式空间不开放写入。';
}).catch(() => {});
validate();

document.getElementById('taskList').addEventListener('click', event => {
  const button = event.target.closest('[data-remove-task]');
  if (button) openTaskRemoval(button.dataset.removeTask);
});
document.getElementById('hideFloatingTaskButton').addEventListener('click', () => removeFloatingTask(false));
document.getElementById('deleteFloatingCloudButton').addEventListener('click', () => removeFloatingTask(true));
document.getElementById('cancelTaskRemovalButton').addEventListener('click', closeTaskRemoval);
document.getElementById('taskRemovalDialog').addEventListener('cancel', event => { if (taskRemovalBusy) event.preventDefault(); else taskRemovalSelection = null; });
document.getElementById('showHiddenFloatingTasks').addEventListener('click', () => restoreHiddenFloatingTasks());

async function submitFormalFloatingFeishu(args) {
  if (args.replacesQueueId != null) throw new Error('WPS 待同步记录不能提交到飞书');
  return call('confirm_floating_feishu_write', args);
}
