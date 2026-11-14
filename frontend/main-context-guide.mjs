import {createContextCoach} from './context-coach.mjs';
let storage;try{storage=localStorage;}catch{/* help remains usable */}
const tracks={
  sentence:[
    {id:'description',target:'#workInput',title:'先说你做了什么',text:'例如“新增整理本周工作计划，重要不紧急，计划明天完成”。这里是旧浮窗的云端记录入口，不是本地工作台表单。'},
    {id:'progress',target:'#progressInput',title:'再填完成后的总进度',text:'数字框仍需填写 0–100 的整数，不是本次增加了多少。即使句子里写了百分比，也要在这里填写。'},
    {id:'create',target:'#createTaskButton',title:'新增时点击这个按钮',text:'点击会分析并生成预览，不等于已写入。先核对名称、日期和分类，再由你确认；本地任务请在工作台里新增。'},
    {id:'update',target:'#updateTaskButton',title:'更新时用已有任务的准确名称',text:'例如“把整理本周工作计划的计划日期改为10月30日”。点击后核对匹配项，找错时手动选择；未通过写入验收时，不要把解析成功当作已更新。',nextLabel:'完成浮窗引导'},
  ],
  feishu:[
    {id:'identity',target:'#feishuAccountConfirmed',title:'先核对飞书身份',text:'在飞书页面选择飞书个人或企业身份，不使用豆包账号。由你核对后勾选；这项声明不能自动识别账号。'},
    {id:'authorize',target:'#authorizeFeishuUserButton',title:'由你点击连接并完成授权',text:'引导不会代点。按页面提示创建应用、扫码、开通并授权。不要发送密钥或含密钥截图；已有连接不必重新建应用。'},
    {id:'name',target:'#feishuWorkspaceNameInput',title:'给任务空间命名',text:'授权后填写名称。默认可用“我的个人任务”；如果已创建空间，优先继续现有连接，避免重复复制模板。'},
    {id:'folder',target:'#feishuWorkspaceFolderLinkInput',title:'保存位置可以留空',text:'留空使用默认位置；需要指定位置时粘贴目标飞书文件夹链接，不用手工查表格编号。'},
    {id:'create',target:'#createFeishuWorkspaceButton',title:'检查后再创建并连接',text:'真实点击才会复制模板。按钮不可用时先完成授权；中断时使用继续或核对入口，不要反复创建。连接成功后回工作台，另行确认同步。',nextLabel:'完成连接引导'},
  ],
};
const coach=createContextCoach({storage,key:'project-track.main-coach.v2',tracks,canStart:()=>!document.getElementById('app').classList.contains('collapsed')});
document.getElementById('recordGuideButton').onclick=()=>coach.start('sentence',true);
document.getElementById('feishuGuideButton').onclick=()=>coach.start('feishu',true);
