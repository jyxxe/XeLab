import {runtimeConfig} from './runtime-config.js';
import {ApiClient} from './api.js';
import {OAuthSession} from './oauth.js';
import {BridgeHost,validateContribution,sameContribution} from './bridge.js';

const $ = selector => document.querySelector(selector);
const node = (tag, props = {}, ...children) => {
  const result = document.createElement(tag);
  for (const [name, value] of Object.entries(props)) {
    if (name.startsWith('on')) result.addEventListener(name.slice(2), value);
    else if (name === 'class') result.className = value;
    else if (name === 'text') result.textContent = value;
    else if (value !== undefined && value !== false) result.setAttribute(name, value === true ? '' : String(value));
  }
  result.append(...children.filter(value => value !== undefined && value !== null));
  return result;
};
const button = (text, action, props = {}) => node('button', {type: 'button', onclick: action, ...props}, text);
const windows = new Map();
const widgetViews = new Map();
const widgetIntents = new Map();
const widgetPinned = new Map();
let layer = 10;
let accountId = null;
let preferences = null;
let preferenceEtag = null;
let toastTimer;
let sessionEpoch=0;
const oauth = new OAuthSession(runtimeConfig);
const api = new ApiClient(runtimeConfig, oauth);
const defaults = () => ({theme: {mode: 'system', reduce_motion: false, reduce_transparency: false}, dock: {position: 'bottom', auto_hide: false, pinned: []}, widgets: [], restore_windows: true});
const baseTokens = {
  light: {accent:'#4F7CFF', desktop:'#E9EEF7', surface:'#FFFFFF', text:'#18202C', muted_text:'#526074', border:'#CBD5E1', danger:'#B42318', radius_px:12, blur_px:16, panel_opacity:0.92, font:'system'},
  dark: {accent:'#80A7FF', desktop:'#121826', surface:'#1F2937', text:'#F3F4F6', muted_text:'#B4C0D0', border:'#475569', danger:'#FF8A80', radius_px:12, blur_px:16, panel_opacity:0.92, font:'system'},
};
let preview = defaults();
const mediaDark = matchMedia('(prefers-color-scheme: dark)');
const mediaMotion = matchMedia('(prefers-reduced-motion: reduce)');

function toast(message) {
  $('#toast').textContent = message;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { $('#toast').textContent = ''; }, 8000);
}
function state(kind, title, message = '') {
  return node('div', {class:'state', 'data-kind':kind, role:kind === 'loading' ? 'status' : 'note'}, node('h3', {}, title), node('p', {}, message));
}
function failure(error) {
  if (error?.name === 'AbortError') return state('unavailable', '等待已停止', '关闭或刷新视图不会回滚已经接受的操作。请使用原请求键核实结果。');
  const status = error.status ?? error.http_status;
  if (status === 401) return state('denied', '登录已失效', '请重新进入内核可信登录页面。');
  if (status === 403) return state('denied', '无权限', `${error.code || 'PERMISSION_DENIED'} · 当前授权来源不允许此操作。`);
  if ([404, 501, 503].includes(status)) return state('unavailable', '当前不可用', `${error.code || 'UNAVAILABLE'} · ${error.message || '接口、资源或依赖尚不可用。'}`);
  return state('error', '请求失败', `${error.code || 'NETWORK_ERROR'} · ${error.message || '连接中断；副作用结果可能未知，请保留原请求键。'}`);
}
function raw(value) { return node('pre', {class:'result'}, JSON.stringify(value, null, 2)); }
function rows(value) { return Array.isArray(value?.items) ? value.items : Array.isArray(value) ? value : []; }
function revision(value) { return value?.revision ? `"rev-${value.revision}"` : undefined; }
function permittedContent(win) {
  if (oauth.loggedIn) return true;
  win.content.replaceChildren(state('denied', '登录后继续', '授权由内核可信页面完成。本页面不会读取平台密码或内核登录 Cookie。'), button('前往可信登录', login, {class:'primary'}));
  return false;
}
async function request(operation, options = {}) {
  const epoch=sessionEpoch;
  try {
    const result = await api.invoke(operation, options);
    if(epoch!==sessionEpoch)throw new Error('登录来源已经改变；旧请求结果不再显示。');
    $('#connection').textContent = '已连接核心';
    return result;
  } catch (error) {
    // ApiClient invalidates only the exact rejected access token. A late 401
    // from an old request must not destroy a newer explicit login.
    if ((error.status ?? error.http_status) === 401 && !oauth.loggedIn) clearPrivateViews();
    $('#connection').textContent = (error.status ?? error.http_status) === 403 ? '当前请求无权限' : '请求未完成';
    throw error;
  }
}
function layoutKey() { return accountId ? `xelab:windows:${runtimeConfig.core_origin}:${runtimeConfig.client_id}:${accountId}` : null; }
function saveLayout() {
  const key = layoutKey();
  if (!key || !preview.restore_windows) return;
  const layout = [...windows.values()].filter(win => !win.ephemeral && !win.contribution).map(win => ({app:win.app.id, rect:win.rect, minimized:win.minimized, maximized:win.maximized}));
  try { localStorage.setItem(key, JSON.stringify(layout)); } catch { /* Storage is optional. */ }
}
function clearPrivateViews() {
  sessionEpoch++;
  const key = layoutKey();
  accountId = null;
  if (key) localStorage.removeItem(key);
  for (const win of [...windows.values()]) closeWindow(win, false);
  for (const widget of widgetViews.values()) widget.dispose();
  widgetViews.clear();widgetIntents.clear();widgetPinned.clear();
  preferences = null;
  preferenceEtag = null;
  preview = defaults();
  applyTheme();
  $('#widgets').replaceChildren();
  $('#connection').textContent = '未连接';
  $('#welcome').hidden = false;
  $('#login').textContent = '登录';
  renderLauncher();
}
oauth.addEventListener('logout', clearPrivateViews);
oauth.addEventListener('credentialchange',()=>{
  for(const win of windows.values())win.bridge?.close('unavailable');
  for(const widget of widgetViews.values())widget.bridge?.close('unavailable');
  toast('访问凭证已轮换；旧插件页面通道已关闭，请先核实未完成的原调用，再明确打开。');
});
async function login() {
  try { await oauth.startLogin(); } catch (error) { toast(`无法进入可信登录：${error.message}`); }
}
function applyTheme() {
  const mode = preview.theme.mode;
  const resolved = mode === 'system' ? (mediaDark.matches ? 'dark' : 'light') : mode;
  const tokens = resolved === 'custom' ? preview.theme.custom_tokens : baseTokens[resolved];
  if (!tokens) return;
  for (const name of ['accent','desktop','surface','text','muted_text','border','danger']) {
    if (/^#[a-f0-9]{6}$/i.test(tokens[name])) document.documentElement.style.setProperty(`--${name.replaceAll('_','-')}`, tokens[name]);
  }
  document.documentElement.style.setProperty('--radius', `${Math.max(0, Math.min(24, tokens.radius_px))}px`);
  document.documentElement.style.setProperty('--blur', `${Math.max(0, Math.min(32, tokens.blur_px))}px`);
  document.documentElement.style.setProperty('--opacity', String(preview.theme.reduce_transparency ? 1 : Math.max(.6, Math.min(1, tokens.panel_opacity))));
  document.documentElement.style.setProperty('--font', ({system:'system-ui,sans-serif',sans:'Arial,sans-serif',serif:'Georgia,serif',mono:'ui-monospace,monospace'})[tokens.font] || 'system-ui,sans-serif');
  document.documentElement.style.colorScheme = resolved === 'dark' ? 'dark' : 'light';
  document.documentElement.dataset.theme = mode;
  document.documentElement.classList.toggle('reduced-motion', preview.theme.reduce_motion || mediaMotion.matches);
  document.documentElement.classList.toggle('reduced-transparency', preview.theme.reduce_transparency);
  $('#dock').dataset.position = preview.dock.position;
  $('#dock').dataset.autoHide = String(preview.dock.auto_hide);
  for(const win of windows.values())win.bridge?.themeChanged();
  for(const widget of widgetViews.values())widget.bridge?.themeChanged();
}
function bridgeTheme(){
  const mode=preview.theme.mode==='system'?(mediaDark.matches?'dark':'light'):preview.theme.mode;
  return{theme_tokens:structuredClone(mode==='custom'?preview.theme.custom_tokens:baseTokens[mode]),reduce_motion:preview.theme.reduce_motion||mediaMotion.matches,reduce_transparency:preview.theme.reduce_transparency};
}
mediaDark.addEventListener('change', applyTheme);
mediaMotion.addEventListener('change', applyTheme);

function workspace() { return {width: $('#windows').clientWidth, height: $('#windows').clientHeight}; }
function fit(win) {
  const area = workspace();
  if (win.maximized) Object.assign(win.element.style, {left:'6px', top:'6px', width:`${Math.max(300, area.width-12)}px`, height:`${Math.max(240, area.height-12)}px`});
  else {
    const r = win.rect;
    const limits=win.contribution?.spec.window;
    r.width = Math.min(area.width-12, Math.max(limits?.min_width||300, Math.min(r.width,limits?.max_width||4096)));
    r.height = Math.min(area.height-12, Math.max(limits?.min_height||240, Math.min(r.height,limits?.max_height||2160)));
    r.x = Math.max(6, Math.min(r.x, Math.max(6, area.width-r.width-6)));
    r.y = Math.max(6, Math.min(r.y, Math.max(6, area.height-r.height-6)));
    Object.assign(win.element.style, {left:`${r.x}px`, top:`${r.y}px`, width:`${r.width}px`, height:`${r.height}px`});
  }
  win.bridge?.viewportChanged();
}
function focus(win) {
  if (matchMedia('(max-width:650px)').matches) for (const other of windows.values()) {other.element.hidden = other !== win;other.bridge?.setVisible(other===win&&document.visibilityState==='visible');}
  win.minimized = false;
  win.bridge?.setVisible(document.visibilityState==='visible');
  win.element.hidden = false;
  if (++layer > 1000) { layer = 10; for (const other of windows.values()) other.element.style.zIndex = String(layer++); }
  for (const other of windows.values()) other.element.dataset.focused = String(other === win);
  win.element.style.zIndex = String(layer);
  $('#active-app').textContent = win.app.title;
  renderDock();
  saveLayout();
}
function closeWindow(win, persist = true) {
  win.controller?.abort();
  win.dispose?.();
  win.element.remove();
  windows.delete(win.id);
  renderDock();
  const next = [...windows.values()].filter(value => !value.minimized).at(-1);
  if (next) focus(next); else $('#active-app').textContent = '桌面';
  if (persist) saveLayout();
}
function pointerMove(win, handle, resize) {
  handle.addEventListener('pointerdown', event => {
    if (event.button !== 0 || (!resize && event.target.closest('button'))) return;
    if (win.maximized || matchMedia('(max-width:650px)').matches||(resize&&win.contribution?.spec.window.resizable===false)) return;
    focus(win);
    const start = {x:event.clientX,y:event.clientY,...win.rect};
    const pointerX = event.clientX, pointerY = event.clientY;
    handle.setPointerCapture(event.pointerId);
    const move = next => {
      if (resize) Object.assign(win.rect, {width:start.width+next.clientX-pointerX,height:start.height+next.clientY-pointerY});
      else Object.assign(win.rect, {x:start.x+next.clientX-pointerX,y:start.y+next.clientY-pointerY});
      fit(win);
    };
    const done = () => { handle.removeEventListener('pointermove',move); handle.removeEventListener('pointerup',done); handle.removeEventListener('pointercancel',done); saveLayout(); };
    handle.addEventListener('pointermove',move);
    handle.addEventListener('pointerup',done);
    handle.addEventListener('pointercancel',done);
  });
}
function openWindow(app, options = {}) {
  if (windows.size >= (runtimeConfig.max_open_windows || 16)) { toast('已达打开窗口上限，请先关闭一个窗口。'); return null; }
  const id = crypto.randomUUID();
  const heading = node('h2', {id:`title-${id}`}, app.title);
  const titlebar = node('header', {class:'window-title', tabindex:0, 'aria-label':`${app.title}窗口。Alt加方向键移动，Alt加Shift加方向键调整大小。`});
  const content = node('div', {class:'window-content'});
  const element = node('section', {class:'window', role:'dialog', 'aria-labelledby':`title-${id}`, 'data-view-id':id, 'data-app':app.id}, titlebar, content);
  const win = {id, app, content, element, rect:options.rect || {x:40+windows.size*24,y:24+windows.size*20,width:680,height:520}, minimized:false,maximized:Boolean(options.maximized),ephemeral:Boolean(options.ephemeral),controller:null};
  const minimize = () => { win.minimized = true; element.hidden = true; win.bridge?.setVisible(false);renderDock(); saveLayout(); };
  const maximize = () => { win.maximized = !win.maximized; fit(win); saveLayout(); };
  titlebar.append(node('div', {class:'window-controls'}, button('×',()=>closeWindow(win),{'aria-label':`关闭${app.title}`}),button('−',minimize,{'aria-label':`最小化${app.title}`}),button('□',maximize,{'aria-label':`最大化或恢复${app.title}`})),heading);
  titlebar.addEventListener('dblclick', event => { if (!event.target.closest('button')) maximize(); });
  titlebar.addEventListener('keydown', event => {
    if (!event.altKey || !['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(event.key)) return;
    if(event.shiftKey&&win.contribution?.spec.window.resizable===false)return;
    event.preventDefault();
    const horizontal = ['ArrowLeft','ArrowRight'].includes(event.key), delta = ['ArrowLeft','ArrowUp'].includes(event.key) ? -20 : 20;
    const key = event.shiftKey ? (horizontal ? 'width' : 'height') : (horizontal ? 'x' : 'y');
    win.rect[key] += delta;
    fit(win);
    saveLayout();
  });
  const resize = button('', ()=>{}, {class:'window-resize', 'aria-label':`调整${app.title}大小`});
  resize.addEventListener('keydown', event => {
    if (!event.key.startsWith('Arrow')) return;
    if(win.contribution?.spec.window.resizable===false)return;
    event.preventDefault();
    win.rect[['ArrowLeft','ArrowRight'].includes(event.key) ? 'width' : 'height'] += ['ArrowLeft','ArrowUp'].includes(event.key) ? -20 : 20;
    fit(win); saveLayout();
  });
  element.append(resize);
  element.addEventListener('pointerdown',()=>focus(win));
  element.addEventListener('focusin',()=> { if (element.dataset.focused !== 'true') focus(win); });
  windows.set(id,win);
  $('#windows').append(element);
  pointerMove(win,titlebar,false);
  pointerMove(win,resize,true);
  fit(win);
  focus(win);
  loadApp(win);
  titlebar.focus();
  if(options.minimized)minimize();
  return win;
}
window.addEventListener('resize',()=> { for (const win of windows.values()){fit(win);if(!matchMedia('(max-width:650px)').matches&&!win.minimized){win.element.hidden=false;win.bridge?.setVisible(document.visibilityState==='visible');}} });

function toolbar(win, title, ...actions) { return node('div',{class:'toolbar'},node('h2',{},title),...actions,button('刷新',()=>loadApp(win))); }
async function loadApp(win) {
  win.controller?.abort();
  win.controller = new AbortController();
  if (!['settings','tasks'].includes(win.app.id) && !permittedContent(win)) return;
  win.content.replaceChildren(state('loading','正在读取核心…'));
  try { await win.app.render(win); } catch (error) { if (windows.has(win.id)) win.content.replaceChildren(toolbar(win,win.app.title),failure(error)); }
}
function editor(title, operation, {params={}, initial={}, etag, body=true, note='操作将以当前用户和本客户端授权提交；服务端会重新鉴权。'} = {}) {
  return openWindow({id:`editor-${operation}`,title,render:async win => {
    const key = crypto.randomUUID();
    const input = node('textarea',{'aria-label':'请求 JSON',spellcheck:false},JSON.stringify(initial,null,2));
    const outcome = node('div',{'aria-live':'polite'});
    const submit = button('提交',async()=> {
      let value;
      try { value = body ? JSON.parse(input.value) : undefined; } catch { outcome.replaceChildren(state('error','JSON 格式错误','请修正后再提交。'));return; }
      submit.disabled = true;
      outcome.replaceChildren(state('loading','请求中…','请保留原请求键，网络失败不等于操作回滚。'));
      try {
        const result = await request(operation,{params,body:value,etag,idempotencyKey:key,signal:win.controller.signal});
        outcome.replaceChildren(node('p',{},'核心已返回以下实际结果；异步 Operation 尚需检查其终态。'),raw(result.data));
      } catch (error) { outcome.replaceChildren(failure(error)); }
      finally { submit.disabled = false; }
    },{class:'primary'});
    win.content.replaceChildren(node('div',{class:'editor-form'},node('p',{class:'muted'},note),...(body?[node('label',{},'请求 JSON',input)]:[]),node('p',{class:'code'},`幂等键：${key}`),submit,outcome));
  }},{ephemeral:true});
}
function details(title, data) { return openWindow({id:'details',title,render:async win=>win.content.replaceChildren(raw(data))},{ephemeral:true}); }
async function listSection(win, {title,operation,query={},actions=[],renderItem}) {
  const result = await request(operation,{query:{limit:50,...query},signal:win.controller.signal});
  const list = node('div',{class:'list'});
  const append = data => { for (const item of rows(data)) list.append(renderItem(item)); };
  append(result.data);
  const more = button('加载下一页',async()=> {
    more.disabled = true;
    try { const next = await request(operation,{query:{limit:50,...query,cursor:more.dataset.cursor},signal:win.controller.signal}); append(next.data); more.dataset.cursor=next.data.next_cursor||'';more.hidden=!next.data.next_cursor; }
    catch(error) { list.append(failure(error)); } finally { more.disabled=false; }
  });
  more.dataset.cursor = result.data.next_cursor || '';
  more.hidden = !result.data.next_cursor;
  if (!list.childElementCount) list.append(state('empty','暂无可见项目','当前账户授权范围内没有记录。'));
  win.content.replaceChildren(toolbar(win,title,...actions),list,more);
}

async function plugins(win) {
  const [installations,deployments] = await Promise.all([
    request('listInstallations',{query:{limit:200},signal:win.controller.signal}),
    request('listDeployments',{query:{limit:200},signal:win.controller.signal}),
  ]);
  const install = button('安装签名镜像',()=>editor('安装插件','installPlugin',{initial:{image:'',publisher_id:'',signature:''}}));
  const deploy = button('创建部署',()=>editor('创建部署','createDeployment',{initial:{installation_id:'',display_name:'',enabled:false,replicas:1,limits:{cpu_millis:500,memory_bytes:536870912,ephemeral_bytes:268435456,pids:128},config:{},volume_ids:[]}}));
  const list = node('div',{class:'list'});
  for (const item of rows(deployments.data)) {
    const spec = item.spec;
    const actions = node('div',{class:'toolbar'},
      button('详情',()=>details('部署详情',item)),
      button(spec.enabled?'停用':'启用',()=>editor('改变部署启停','setDeploymentEnabled',{params:{deployment_id:item.id},initial:{enabled:!spec.enabled},etag:revision(item),note:'停用会撤销新路由。核心 API 与 CLI 仍可独立使用。'})),
      button('审批',()=>editor('审批插件版本','approvePlugin',{params:{deployment_id:item.id},initial:{rules:[],egress:[]},etag:revision(item)})),
      button('配置',()=>editor('修改部署','updateDeployment',{params:{deployment_id:item.id},initial:spec,etag:revision(item)})));
    list.append(node('article',{class:'card'},node('h3',{},spec.display_name),node('p',{},node('span',{class:'badge'},item.phase),` · ${spec.enabled?'已启用':'已停用'}`),node('p',{class:'code'},item.id),node('p',{},(item.conditions||[]).join(' · ')),actions));
  }
  for (const item of rows(installations.data)) list.append(node('article',{class:'card'},node('h3',{},item.plugin_id),node('p',{},`版本 ${item.version} · ${item.state}`),node('p',{class:'code'},item.image_digest),node('div',{class:'toolbar'},button('安装详情',()=>details('固定安装制品',item)))));
  if (!list.childElementCount) list.append(state('empty','暂无可见插件','安装签名制品后，创建关闭状态的部署并审批。'));
  win.content.replaceChildren(toolbar(win,'插件中心',install,deploy),list);
  if (installations.data.next_cursor || deployments.data.next_cursor) win.content.append(state('unavailable','还有更多项目','本视图最多显示每类200项。请使用 CLI 分页或缩小核心授权范围。'));
}
async function skills(win) {
  await listSection(win,{title:'Skill 浏览器',operation:'listSkills',actions:[button('注册固定 Skill',()=>editor('注册 Skill','registerSkill',{initial:{installation_id:'',definition:{},enabled:false}}))],renderItem:item=>node('article',{class:'card'},node('h3',{},item.definition.name),node('p',{},`${item.definition.version} · ${item.availability}`),node('p',{class:'code'},`UUID ${item.id}`),node('p',{class:'code'},item.definition_digest),node('p',{},(item.reason_codes||[]).join(' · ')),node('div',{class:'toolbar'},button('定义与输入 Schema',()=>details('不可变 Skill 定义',item)),button('调用测试',()=>editor('调用固定 Skill','invokeSkill',{params:{skill_id:item.id},initial:{input:{},idempotency_key:crypto.randomUUID()},note:`固定UUID ${item.id}；定义摘要 ${item.definition_digest}。输入须符合此版本Schema，权限仍由核心逐次校验。`})),button(item.enabled?'禁用':'启用',()=>editor('改变 Skill 状态','setSkillEnabled',{params:{skill_id:item.id},initial:{enabled:!item.enabled},etag:revision(item)})),button('归档',()=>editor('归档 Skill','archiveSkill',{params:{skill_id:item.id},etag:revision(item),body:false,note:'归档是不可逆的逻辑停用；历史调用仍保留。'}))))});
}
async function accounts(win) {
  const accountInput = node('input',{placeholder:'账户 UUID','aria-label':'账户 UUID',value:accountId||''});
  const display = node('div');
  const inspect = async id => {
    display.replaceChildren(state('loading','读取账户及授权来源…'));
    try {
      const result = await request('getAccount',{params:{account_id:id},signal:win.controller.signal});
      display.replaceChildren(node('h3',{},`${result.data.display_name} · ${result.data.status}`),raw(result.data));
      const read = (title,operation) => button(title,async()=> { try { const data=await request(operation,{params:{account_id:id},signal:win.controller.signal});details(title,data.data); } catch(error){toast(`${error.code||'请求失败'}：${error.message}`);} });
      display.append(node('div',{class:'toolbar'},read('有效权限与阻断祖先','getEffectivePermissions'),read('授权组来源','listGrants'),read('资源配额','getQuota'),button('子账户',async()=> { try { const data=await request('listAccounts',{query:{parent_id:id,limit:200},signal:win.controller.signal});details('子账户与继承模式',data.data); }catch(error){toast(error.message);} })));
    } catch(error) {display.replaceChildren(failure(error));}
  };
  win.content.replaceChildren(toolbar(win,'账户与权限'),node('p',{class:'muted'},'账户身份、祖先限制与凭证来源均以核心返回为准；WebUI 不收集平台密码。'),node('div',{class:'inline-field'},accountInput,button('读取',()=>inspect(accountInput.value.trim()))),display);
  if (accountId) await inspect(accountId); else display.replaceChildren(state('unavailable','当前账户详情尚不可用','当前委托未获准读取本人会话元数据。可输入已知账户 UUID，在现有授权内查询。'));
}
async function resources(win) {
  const result = await request('getNode',{signal:win.controller.signal});
  win.content.replaceChildren(toolbar(win,'资源与系统状态'),node('p',{class:'muted'},'以下为核心实际返回的宿主状态与资源账本。内核4C16G预留不借给界面。'),raw(result.data));
}
async function clients(win) {
  await listSection(win,{title:'UI 客户端',operation:'listClients',actions:[button('注册公共客户端',()=>editor('注册 UI Client','createClient',{initial:{name:'',plugin_id:'',redirect_uris:[],allowed_origins:[],maximum_rules:[]}})),button('默认界面',async()=> { try { const result=await request('getDefaultUi',{signal:win.controller.signal});editor('设置默认界面','setDefaultUi',{initial:{deployment_id:result.data.deployment_id??null},etag:result.etag||revision(result.data),note:'设为 null 关闭根路径跳转。修改前请保留部署说明中的内核 CLI 恢复命令。'}); }catch(error){toast(error.message);} })],renderItem:item=>node('article',{class:'card'},node('h3',{},item.spec.name),node('p',{},item.status),node('p',{class:'code'},item.client_id),node('p',{},item.spec.allowed_origins.join(', ')),node('div',{class:'toolbar'},button('详情',()=>details('Client 信任范围',item)),button('修改',()=>editor('修改 Client','updateClient',{params:{client_id:item.id},initial:item.spec,etag:revision(item),note:'修改将作废旧授权请求与本Client现有凭证族。禁用Client不能通过修改复活。'})),button('禁用',()=>editor('禁用 Client','disableClient',{params:{client_id:item.id},body:false,etag:revision(item),note:'禁用会撤销本Client凭证族；保留核心 API/CLI 恢复路径。'}))))});
}
async function openContribution(value){
  if(!accountId||!oauth.loggedIn){toast('打开页面需要当前账户的 session.read 权限以固定桥接主体。');return null;}
  const current=await request('getUiContribution',{params:{deployment_id:value.deployment_id,contribution_id:value.spec.id}});
  const fixed=validateContribution(current.data,{parentOrigin:location.origin,coreOrigin:runtimeConfig.core_origin});
  if(!sameContribution(value,fixed))throw new Error('贡献或权限已改变，请刷新后明确打开当前版本。');
  if(fixed.spec.kind!=='window')throw new Error('该贡献不是窗口。');
  return openWindow({id:`plugin:${fixed.deployment_id}:${fixed.spec.id}`,title:`${fixed.spec.title} · ${fixed.plugin_id}`,render:async win=>{
    win.contribution=fixed;
    win.rect.width=fixed.spec.window.width;win.rect.height=fixed.spec.window.height;fit(win);
    const resize=win.element.querySelector('.window-resize');resize.hidden=!fixed.spec.window.resizable;
    await mountContribution(win.content,fixed,win);
  }},{ephemeral:true});
}
async function mountContribution(container,fixed,owner){
  const frame=node('iframe',{class:'bridge-frame'}),status=node('p',{class:'contribution-note'},'正在核对固定贡献并握手…');
  container.replaceChildren(node('p',{class:'contribution-note',title:`安装 ${fixed.installation_id}；清单 ${fixed.manifest_digest}`},`固定贡献 ${fixed.contribution_digest.slice(0,23)}…`),status,frame);
  const bridge=new BridgeHost({iframe:frame,api,contribution:fixed,subjectId:accountId,isCurrent:subject=>oauth.loggedIn&&accountId===subject,coreOrigin:runtimeConfig.core_origin,viewId:owner.id,theme:bridgeTheme,
    openWindow:async contribution=>(await openContribution(contribution))?.id,
    onState:(phase,reason)=>{if(phase==='active')status.hidden=true;else if(phase==='closed'){owner.bridge=null;owner.closed=true;clearInterval(owner.refreshTimer);container.replaceChildren(state('unavailable','页面通道已关闭',`${reason} · 请重新发现并明确打开当前版本。`));}},
  });
  owner.bridge=bridge;
  const observer=new ResizeObserver(()=>bridge.viewportChanged());observer.observe(frame);
  owner.dispose=()=>{observer.disconnect();clearInterval(owner.refreshTimer);bridge.close('close');};
  await bridge.mount();
  bridge.setVisible(document.visibilityState==='visible'&&!owner.minimized);
}
async function widgetMutation(intentId,mutate){
  let intent=widgetIntents.get(intentId);
  if(!intent){const current=await request('getMyUiPreferences');const body=structuredClone(current.data.spec);mutate(body.widgets);intent={body,etag:current.etag||revision(current.data),key:crypto.randomUUID()};widgetIntents.set(intentId,intent);}
  const saved=await request('setMyUiPreferences',{body:intent.body,etag:intent.etag,idempotencyKey:intent.key});
  widgetIntents.delete(intentId);preferences=saved.data;preferenceEtag=saved.etag||revision(saved.data);preview=structuredClone(saved.data.spec);applyTheme();await renderWidgets();
}
async function addWidget(value){
  if(!accountId||!oauth.loggedIn){toast('组件需要读取当前账户身份与核心偏好。');return;}
  const current=await request('getUiContribution',{params:{deployment_id:value.deployment_id,contribution_id:value.spec.id}});
  const fixed=validateContribution(current.data,{parentOrigin:location.origin,coreOrigin:runtimeConfig.core_origin});
  if(!sameContribution(value,fixed))throw new Error('贡献或权限已改变，请刷新后重新选择。');
  if(fixed.spec.kind!=='widget')throw new Error('该贡献不是组件。');
  await widgetMutation(`add:${fixed.deployment_id}:${fixed.spec.id}`,items=>{if(items.length>=32)throw new Error('布局最多32项。');const id=crypto.randomUUID();widgetPinned.set(id,fixed);items.push({instance_id:id,contribution:{deployment_id:fixed.deployment_id,contribution_id:fixed.spec.id},column:0,row:items.length*2,columns:fixed.spec.widget.columns,rows:fixed.spec.widget.rows});});
}
async function renderWidgets(){
  if(!oauth.loggedIn||!accountId)return;
  const placements=preview.widgets||[],ids=new Set(placements.map(item=>item.instance_id));
  for(const[id,widget]of widgetViews)if(!ids.has(id)){widget.dispose();widget.element.remove();widgetViews.delete(id);widgetPinned.delete(id);}
  for(const placement of placements){
    const key=JSON.stringify(placement.contribution);let widget=widgetViews.get(placement.instance_id);
    if(widget&&widget.key!==key){widget.dispose();widget.element.remove();widgetViews.delete(placement.instance_id);widget=null;}
    if(widget){widget.element.style.gridColumn=`${placement.column+1}/span ${placement.columns}`;widget.element.style.gridRow=`${placement.row+1}/span ${placement.rows}`;continue;}
    const card=node('article',{class:'card widget-card'}),body=node('div',{class:'widget-content'});
    const remove=button('移除',async()=>{try{await widgetMutation(`remove:${placement.instance_id}`,items=>{const index=items.findIndex(item=>item.instance_id===placement.instance_id);if(index>=0)items.splice(index,1);});}catch(error){toast(error.message);}});
    card.append(node('div',{class:'toolbar'},node('h3',{},'桌面组件'),remove),body);card.style.gridColumn=`${placement.column+1}/span ${placement.columns}`;card.style.gridRow=`${placement.row+1}/span ${placement.rows}`;
    $('#widgets').append(card);
    widget={id:crypto.randomUUID(),key,element:card,dispose:()=>{},bridge:null,visible:true};widgetViews.set(placement.instance_id,widget);
    const retry=button('明确重新加载',async()=>{widget.dispose();card.remove();widgetViews.delete(placement.instance_id);widgetPinned.delete(placement.instance_id);await renderWidgets();});
    card.querySelector('.toolbar').append(retry);
    if([...widgetViews.values()].filter(item=>item.bridge).length>=(runtimeConfig.max_active_widgets??8)){body.replaceChildren(state('unavailable','等待激活','活动组件已达部署上限；移除其他活动组件后明确重新加载。'));continue;}
    body.replaceChildren(state('loading','读取获准组件…'));
    try{
      const result=await request('getUiContribution',{params:placement.contribution});
      const fixed=validateContribution(result.data,{parentOrigin:location.origin,coreOrigin:runtimeConfig.core_origin});
      if(fixed.spec.kind!=='widget')throw new Error('布局引用不是获准组件。');
      if(widgetPinned.has(placement.instance_id)&&!sameContribution(widgetPinned.get(placement.instance_id),fixed))throw new Error('固定贡献发生变化，请明确重新加载。');
      widgetPinned.set(placement.instance_id,fixed);
      card.querySelector('h3').textContent=fixed.spec.title;
      await mountContribution(body,fixed,widget);
      const visible=new IntersectionObserver(entries=>{for(const entry of entries)setWidgetVisible(widget,entry.isIntersecting&&document.visibilityState==='visible');});visible.observe(card);
      const cleanup=widget.dispose;widget.dispose=()=>{visible.disconnect();cleanup();};
      card.querySelector('.toolbar').append(button('刷新',()=>widget.bridge?.refresh().catch(()=>{})));
      if((fixed.spec.refresh_interval_seconds??0)>0){
        try{await verifyPureWidget(fixed);widget.refreshSeconds=Math.max(fixed.spec.refresh_interval_seconds,runtimeConfig.widget_min_refresh_seconds||30);setWidgetVisible(widget,widget.visible);}
        catch{card.append(node('p',{class:'contribution-note'},'当前来源不足以证明纯只读目标；周期刷新未开启。'));}
      }
    }catch(error){body.replaceChildren(failure(error));}
  }
}
async function verifyPureWidget(fixed){
  const bindings=fixed.bindings.filter(binding=>binding.modes.includes('call'));
  if(!bindings.length)throw new Error('缺少已批准读取。');
  const installed=await request('getInstallation',{params:{installation_id:fixed.installation_id}});
  for(const binding of bindings){
    const {data:skill}=await request('getSkill',{params:{skill_id:binding.skill_id}});
    const target=skill.definition.target;
    if(skill.id!==binding.skill_id||skill.definition_digest!==binding.definition_digest||skill.installation_id!==fixed.installation_id||!skill.enabled||skill.availability!=='ready'||target.kind!=='capability')throw new Error('读取绑定无法固定。');
    const capability=installed.data.manifest.capabilities.find(item=>item.contract.name===target.contract.name&&item.contract.version===target.contract.version&&item.contract.schema_digest===target.contract.schema_digest);
    if(capability?.side_effect!=='none')throw new Error('目标不是纯读取。');
  }
}
function setWidgetVisible(widget,visible){widget.visible=visible;widget.bridge?.setVisible(visible);clearInterval(widget.refreshTimer);if(visible&&widget.refreshSeconds&&widget.bridge)widget.refreshTimer=setInterval(()=>widget.bridge?.refresh().catch(()=>{}),widget.refreshSeconds*1000);}
document.addEventListener('visibilitychange',()=>{for(const win of windows.values())win.bridge?.setVisible(!win.minimized&&!win.element.hidden&&document.visibilityState==='visible');for(const widget of widgetViews.values())setWidgetVisible(widget,document.visibilityState==='visible'&&widget.element.getBoundingClientRect().bottom>0&&widget.element.getBoundingClientRect().top<innerHeight);});
async function contributions(win) {
  await listSection(win,{title:'插件页面',operation:'listUiContributions',renderItem:item=>node('article',{class:'card'},node('h3',{},item.spec.title||item.spec.id),node('p',{},`${item.spec.kind} · ${item.availability}`),node('p',{class:'code'},item.contribution_digest),node('p',{},item.reason_codes.join(' · ')),node('div',{class:'toolbar'},button('固定贡献详情',()=>details('已批准 UI 贡献',item)),button(item.spec.kind==='widget'?'添加桌面组件':'打开隔离窗口',async()=>{try{if(item.spec.kind==='widget')await addWidget(item);else await openContribution(item);}catch(error){toast(error.message);}},{disabled:item.availability!=='ready'})))});
}
async function settings(win) {
  if (oauth.loggedIn) {
    try { const result=await request('getMyUiPreferences',{signal:win.controller.signal});preferences=result.data;preferenceEtag=result.etag||revision(result.data);preview=structuredClone(result.data.spec);applyTheme(); }
    catch(error) { toast(`核心偏好暂不可用：${error.code||error.message}。主题更改将仅为预览。`); }
  }
  const notice = node('p',{class:'theme-preview-note'},preferences?'已读取当前用户核心偏好。':'核心偏好未加载；以下仅为本页预览，未持久保存。');
  const form = node('div',{class:'settings-grid'});
  const mode = node('select',{'aria-label':'主题模式'},...['system','light','dark','custom'].map(value=>node('option',{value},({system:'跟随系统',light:'明亮',dark:'深色',custom:'自定义'})[value])));
  mode.value=preview.theme.mode;
  const colors=node('div',{class:'theme-colors'});
  const mark = () => { notice.textContent='当前为未保存预览。';applyTheme(); };
  function customFields() {
    colors.replaceChildren();colors.hidden=preview.theme.mode!=='custom';
    if (colors.hidden) return;
    preview.theme.custom_tokens ||= structuredClone(baseTokens[mediaDark.matches?'dark':'light']);
    for (const name of ['accent','desktop','surface','text','muted_text','border','danger']) {
      const input=node('input',{type:'color',value:preview.theme.custom_tokens[name],'aria-label':`自定义 ${name}`});
      input.addEventListener('input',()=> { preview.theme.custom_tokens[name]=input.value;mark(); });
      colors.append(node('label',{},name,input));
    }
  }
  mode.addEventListener('change',()=> { preview.theme.mode=mode.value;if(mode.value!=='custom')delete preview.theme.custom_tokens;customFields();mark(); });
  const check = (title,key) => { const input=node('input',{type:'checkbox','aria-label':title});input.checked=preview.theme[key];input.addEventListener('change',()=> { preview.theme[key]=input.checked;mark(); });return node('label',{class:'check'},input,title); };
  const dock=node('select',{'aria-label':'Dock 位置'},...['bottom','left','right'].map(value=>node('option',{value},({bottom:'底部',left:'左侧',right:'右侧'})[value])));dock.value=preview.dock.position;dock.addEventListener('change',()=>{preview.dock.position=dock.value;mark();});
  const autoHide=node('input',{type:'checkbox','aria-label':'自动隐藏 Dock'});autoHide.checked=preview.dock.auto_hide;autoHide.addEventListener('change',()=>{preview.dock.auto_hide=autoHide.checked;mark();});
  let saveIntent = {key:crypto.randomUUID(), body:null};
  const save=button('保存到当前账户',async()=> {
    if (!preferences || !preferenceEtag) {toast('需要先登录并成功读取核心偏好及其ETag。');return;}
    const canonicalIntent=JSON.stringify(preview);
    if(saveIntent.body!==null&&saveIntent.body!==canonicalIntent)saveIntent={key:crypto.randomUUID(),body:null};
    saveIntent.body=canonicalIntent;
    save.disabled=true;
    try {const result=await request('setMyUiPreferences',{body:preview,etag:preferenceEtag,idempotencyKey:saveIntent.key,signal:win.controller.signal});preferences=result.data;preferenceEtag=result.etag||revision(result.data);preview=structuredClone(result.data.spec);applyTheme();notice.textContent='已保存到核心，重新登录后仍可读取。';saveIntent={key:crypto.randomUUID(),body:null};}
    catch(error){notice.replaceChildren(failure(error));}finally{save.disabled=false;}
  },{class:'primary'});
  form.append(node('label',{},'主题',mode),colors,check('减少动画','reduce_motion'),check('减少透明度','reduce_transparency'),node('label',{},'Dock 位置',dock),node('label',{class:'check'},autoHide,'自动隐藏 Dock'),notice,node('div',{class:'toolbar'},save,button('恢复已保存偏好',()=> {preview=preferences?structuredClone(preferences.spec):defaults();applyTheme();loadApp(win);})),node('p',{class:'code'},`核心：${runtimeConfig.core_origin}\n公共 Client：${runtimeConfig.client_id}`));
  customFields();
  win.content.replaceChildren(toolbar(win,'桌面设置'),form);
}
async function unavailable(win) {win.content.replaceChildren(toolbar(win,win.app.title),state('unavailable','当前版本尚不可用','Batch、Workflow、微插件与知识库属于后续执行系统。此桌面未伪造任务、数据或成功响应。'));}
async function tasks(win) {
  const input=node('input',{'aria-label':'Operation UUID',placeholder:'已接受操作的 UUID'});
  const result=node('div');
  win.content.replaceChildren(toolbar(win,'任务与能力'),node('p',{},'查询核心已经接受的 Operation；结果以其持久终态为准。'),node('div',{class:'inline-field'},input,button('查询 Operation',async()=>{result.replaceChildren(state('loading','查询中…'));try{const response=await request('getOperation',{params:{operation_id:input.value.trim()},signal:win.controller.signal});result.replaceChildren(raw(response.data));}catch(error){result.replaceChildren(failure(error));}})),result,state('unavailable','Batch / Workflow / 微插件 / 知识库尚不可用','未安装或实现的后续执行系统没有可展示的运行结果。'));
}
const apps = [
  {id:'plugins',title:'插件中心',icon:'◈',render:plugins},
  {id:'skills',title:'Skill',icon:'S',render:skills},
  {id:'accounts',title:'账户与权限',icon:'◎',render:accounts},
  {id:'resources',title:'系统状态',icon:'▥',render:resources},
  {id:'clients',title:'UI 客户端',icon:'⌘',render:clients},
  {id:'contributions',title:'插件页面',icon:'▧',render:contributions},
  {id:'tasks',title:'任务与知识库',icon:'≡',render:tasks},
  {id:'settings',title:'设置',icon:'⚙',render:settings},
];
function renderDock() {
  const dock=$('#dock');dock.replaceChildren();
  for (const app of apps.filter(value=>!['clients','contributions','tasks'].includes(value.id))) {
    const active=[...windows.values()].filter(win=>win.app.id===app.id);
    dock.append(button('',()=> { const minimized=active.find(win=>win.minimized||win.element.hidden);if(minimized)focus(minimized);else openWindow(app); },{'aria-label':`打开${app.title}`,'data-app-id':app.id,'data-active':String(active.length>0)}));
    dock.lastChild.append(node('span',{class:'app-icon','aria-hidden':true},app.icon),node('span',{},app.title));
  }
  for (const win of windows.values()) if(win.minimized) dock.append(button(`↥ ${win.app.title}`,()=>focus(win),{'aria-label':`恢复${win.app.title}`}));
  dock.append(button('应用',()=>{renderLauncher();$('#launcher').showModal();},{'aria-label':'打开应用启动器'}));
}
function renderLauncher() {
  const search=$('#app-search').value.trim().toLowerCase();
  $('#app-grid').replaceChildren(...apps.filter(app=>app.title.toLowerCase().includes(search)||app.id.includes(search)).map(app=>button('',()=>{$('#launcher').close();openWindow(app);},{'aria-label':`启动${app.title}`})));
  for (const [index,entry] of [...$('#app-grid').children].entries()) {
    const matching=apps.filter(app=>app.title.toLowerCase().includes(search)||app.id.includes(search))[index];
    entry.append(node('span',{class:'app-icon','aria-hidden':true},matching.icon),node('span',{},matching.title));
  }
  for(const[ index,win]of[...windows.values()].entries())if(win.app.title.toLowerCase().includes(search))$('#app-grid').append(button(`↥ ${win.app.title}`,()=>{$('#launcher').close();focus(win);},{'aria-label':`聚焦窗口${index+1} ${win.app.title}`}));
}
$('#launcher-toggle').addEventListener('click',()=>{renderLauncher();$('#launcher').showModal();});
$('#launcher-close').addEventListener('click',()=>$('#launcher').close());
$('#app-search').addEventListener('input',renderLauncher);
$('#theme-toggle').addEventListener('click',()=>openWindow(apps.find(app=>app.id==='settings')));
$('#welcome-login').addEventListener('click',login);
$('#login').addEventListener('click',()=> { if(oauth.loggedIn){oauth.logout();clearPrivateViews();toast('已退出本WebUI客户端；内核可信登录会话由内核独立管理。');}else login(); });
document.addEventListener('keydown',event=> {if(event.key==='Escape'&&$('#launcher').open)$('#launcher').close();});
function tick() {$('#clock').textContent=new Intl.DateTimeFormat('zh-CN',{hour:'2-digit',minute:'2-digit'}).format(new Date());}
tick();setInterval(tick,30000);
renderDock();renderLauncher();applyTheme();
async function boot() {
  try {
    if (location.pathname === '/ui/org.xelab.webui/callback') {
      await oauth.finishLogin(location.href);
      history.replaceState({},'', '/ui/org.xelab.webui/');
    }
    if (!oauth.loggedIn) return;
    $('#login').textContent='退出';$('#welcome').hidden=true;$('#connection').textContent='正在验证连接…';
    try {
      const sessions=await request('listSessions',{query:{limit:200}});
      const subjects=[...new Set(rows(sessions.data).filter(item=>item.status==='active').map(item=>item.account_id))];
      if(subjects.length===1) accountId=subjects[0];
    } catch { /* Authorization may intentionally exclude session metadata. */ }
    try {const result=await request('getMyUiPreferences');preferences=result.data;preferenceEtag=result.etag||revision(result.data);preview=structuredClone(result.data.spec);applyTheme();await renderWidgets();}catch{ /* Settings reports the unavailable dependency. */ }
    const key=layoutKey();
    if(key&&preview.restore_windows) {
      try {const restored=JSON.parse(localStorage.getItem(key)||'[]');if(Array.isArray(restored))for(const item of restored.slice(0,runtimeConfig.max_open_windows||16)){const app=apps.find(candidate=>candidate.id===item.app);if(app&&item.rect&&['x','y','width','height'].every(key=>Number.isFinite(item.rect[key])))openWindow(app,item);}}catch{/* Invalid optional local layout is discarded. */}
    }
  }catch(error){clearPrivateViews();toast(`登录未完成：${error.code||error.message}`);}
}
boot();
