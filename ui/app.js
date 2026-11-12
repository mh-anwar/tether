'use strict';
// Surface unexpected script errors instead of leaving a frozen screen.
window.addEventListener('error', (e) => reportError(e.message));
window.addEventListener('unhandledrejection', (e) => reportError(String(e.reason)));
function reportError(msg) {
  const el = document.getElementById('toasts');
  if (!el) return;
  const t = document.createElement('div');
  t.className = 'toast error';
  t.textContent = `Something went wrong: ${msg}`;
  el.append(t);
  setTimeout(() => t.remove(), 8000);
}
// Tether window. All phone data comes from the daemon through `invoke('call', ...)`; events arrive
// on the 'tether' channel. Text from the phone is always inserted with textContent, never as HTML.

const T = window.__TAURI__;
const api = (method, params = {}) => T.core.invoke('call', { method, params });
const $ = (sel, root = document) => root.querySelector(sel);

const state = {
  daemon: false,
  update: null, // update_status from the daemon
  bootFailed: false,
  me: null,
  devices: [],
  deviceId: null,
  battery: null,
  signal: {},
  notifications: {},
  threads: new Map(), // thread_id -> Map(message id -> message)
  contacts: [], // [{name, number}] numbers are digits only
  selected: null, // thread id, or 'new'
  filter: '',
  pending: [], // outgoing texts not yet confirmed by the phone
  loadingThread: null,
  call: null,
  media: {}, // player name -> what it plays, from the phone
  feedOverlay: false,
  newTo: [],
  seen: loadSeen(),
  view: 'messages', // or 'apps'
};

function loadSeen() {
  try { return JSON.parse(localStorage.getItem('tether.seen') || '{}'); } catch { return {}; }
}
function saveSeen() {
  try { localStorage.setItem('tether.seen', JSON.stringify(state.seen)); } catch { /* private mode */ }
}

// ---------- tiny DOM helper ----------
function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'text') el.textContent = v;
    else if (k.startsWith('on')) el.addEventListener(k.slice(2).toLowerCase(), v);
    else if (k === 'style') el.style.cssText = v;
    else el.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

/** Re-renders a region while keeping focus, typed text and caret in inputs that carry data-key. */
function swap(container, build) {
  if (!container) return;
  const active = document.activeElement;
  const key = active && container.contains(active) ? active.dataset.key : null;
  const saved = key ? { value: active.value, start: active.selectionStart, end: active.selectionEnd } : null;
  const scroll = container.scrollTop;
  container.replaceChildren(...[build()].flat().filter(Boolean));
  container.scrollTop = scroll;
  if (key) {
    const el = container.querySelector(`[data-key="${CSS.escape(key)}"]`);
    if (el) {
      if ('value' in el && saved.value !== undefined) el.value = saved.value;
      el.focus();
      try { el.setSelectionRange(saved.start, saved.end); } catch { /* not a text field */ }
    }
  }
}

// ---------- formatting ----------
const digits = (s) => String(s || '').replace(/\D/g, '');

function nameFor(number) {
  const d = digits(number);
  if (!d) return number;
  const tail = d.slice(-9);
  const hit = state.contacts.find((c) => c.number === d) ||
    (tail.length >= 7 ? state.contacts.find((c) => c.number.slice(-9) === tail) : null);
  return hit ? hit.name : prettyNumber(number);
}

function prettyNumber(n) {
  const d = digits(n);
  if (d.length === 11 && d.startsWith('1')) return `(${d.slice(1, 4)}) ${d.slice(4, 7)}-${d.slice(7)}`;
  if (d.length === 10) return `(${d.slice(0, 3)}) ${d.slice(3, 6)}-${d.slice(6)}`;
  return n;
}

function threadTitle(addresses) {
  const names = (addresses || []).map((a) => nameFor(a.address));
  if (names.length <= 2) return names.join(', ') || 'Unknown';
  return `${names.slice(0, 2).join(', ')} and ${names.length - 2} more`;
}

function initials(title) {
  const words = title.replace(/[^\p{L}\p{N} ]/gu, '').trim().split(/\s+/);
  if (!words[0]) return '#';
  if (/^\d/.test(words[0])) return '#';
  return (words[0][0] + (words[1] ? words[1][0] : '')).toUpperCase();
}

function avatar(title, group, size = '') {
  let hash = 0;
  for (const ch of title) hash = (hash * 31 + ch.codePointAt(0)) >>> 0;
  const hue = hash % 360;
  return h('div', { class: `avatar ${group ? 'group' : ''} ${size}`, style: `background:hsl(${hue} 45% 46%)`, 'aria-hidden': 'true' },
    group ? icon('message', 'sm') : initials(title));
}

const sameDay = (a, b) => a.toDateString() === b.toDateString();
function shortTime(ms) {
  const d = new Date(ms), now = new Date();
  if (sameDay(d, now)) return d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  const y = new Date(now); y.setDate(now.getDate() - 1);
  if (sameDay(d, y)) return 'Yesterday';
  if (now - d < 6 * 864e5) return d.toLocaleDateString([], { weekday: 'short' });
  return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
}
function dayLabel(ms) {
  const d = new Date(ms), now = new Date();
  if (sameDay(d, now)) return 'Today';
  const y = new Date(now); y.setDate(now.getDate() - 1);
  if (sameDay(d, y)) return 'Yesterday';
  if (now - d < 6 * 864e5) return d.toLocaleDateString([], { weekday: 'long' });
  return d.toLocaleDateString([], { month: 'long', day: 'numeric', year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric' });
}
function ago(ms) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return 'now';
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  return shortTime(ms);
}

// One-time codes: "code is 482913", "482913 is your code", "G-482913".
const CODE_RES = [
  /\b(?:code|otp|passcode|pin|password|verification|token|kod|codigo|código)\b[^\d\n]{0,40}?\b(\d{4,8})\b/i,
  /\b(\d{4,8})\b[^\d\n]{0,30}\b(?:is your|is the|as your)\b[^\n]{0,30}\b(?:code|otp|pin|password)\b/i,
  /\b[A-Z]-(\d{4,8})\b/,
];
function findCode(text) {
  for (const re of CODE_RES) {
    const m = re.exec(text || '');
    if (m) return m[1];
  }
  return null;
}
const findLinks = (text) => (text || '').match(/https?:\/\/[^\s<>"]+/g) || [];

// ---------- data ----------
function device() { return state.devices.find((d) => d.id === state.deviceId) || null; }

function mergeMessages(list) {
  for (const m of list) {
    if (!state.threads.has(m.thread_id)) state.threads.set(m.thread_id, new Map());
    state.threads.get(m.thread_id).set(m._id, m);
    if (m.type === 2) {
      // The phone confirmed a text we sent: drop the matching "Sending" bubble.
      const i = state.pending.findIndex((p) => p.body === m.body && (p.thread === m.thread_id || sameAddrs(p.addresses, m.addresses)));
      if (i >= 0) state.pending.splice(i, 1);
    }
  }
  if (state.selected === 'new' && state.newTo.length) {
    const hit = list.find((m) => sameAddrs(state.newTo, m.addresses));
    if (hit) { state.selected = hit.thread_id; state.newTo = []; renderThreadPane(true); }
  }
}

function sameAddrs(a, b) {
  const norm = (x) => (x || []).map((v) => digits(v.address ?? v).slice(-9)).sort().join('|');
  return norm(a) === norm(b);
}

function threadMessages(id) {
  const t = state.threads.get(id);
  return t ? [...t.values()].sort((a, b) => a.date - b.date) : [];
}

function heads() {
  const out = [];
  for (const [id, t] of state.threads) {
    let last = null;
    for (const m of t.values()) if (!last || m.date > last.date) last = m;
    if (last) out.push(last);
  }
  return out.sort((a, b) => b.date - a.date);
}

function isUnread(head) {
  return head.type === 1 && head.read === 0 && head.date > (state.seen[head.thread_id] || 0);
}

async function loadAll() {
  try {
    state.me = await api('identity');
    state.devices = await api('devices');
  } catch (e) {
    return toast(String(e), 'error');
  }
  const paired = state.devices.filter((d) => d.pair === 'paired');
  if (!paired.find((d) => d.id === state.deviceId)) {
    state.deviceId = (paired.find((d) => d.connected) || paired[0] || {}).id || null;
    state.threads.clear();
    state.selected = null;
  }
  loadUpdate();
  if (state.deviceId) await loadDevice();
  render();
}

async function loadDevice() {
  const id = state.deviceId;
  try {
    const [data, threads, contacts] = await Promise.all([
      api('device_data', { device: id }), api('threads', { device: id }), api('contacts', { device: id }),
    ]);
    if (state.deviceId !== id) return; // another phone was picked meanwhile
    state.battery = data.battery;
    state.signal = data.signal || {};
    state.notifications = data.notifications || {};
    state.call = data.call;
    state.media = data.media || {};
    state.contacts = contacts;
    mergeMessages(threads);
  } catch (e) {
    toast(String(e), 'error');
  }
}

// ---------- rendering ----------
function render() {
  const app = $('#app');
  app.removeAttribute('aria-busy');
  if (!state.daemon) {
    app.className = 'app single';
    swap(app, () => h('div', { class: 'boot' },
      state.bootFailed ? icon('alert', 'lg') : h('div', { class: 'spinner', 'aria-hidden': 'true' }),
      h('p', { text: state.bootFailed ? "Tether's background service isn't running." : 'Starting Tether' }),
      state.bootFailed && h('p', { class: 'hint', text: 'It is restarted automatically. If this stays, run "tether daemon" once to see why.' })));
    return;
  }
  if (!state.deviceId) return renderPairPage(app);
  if (!$('.rail', app)) buildLayout(app);
  app.className = `app ${state.feedOverlay ? 'feed-overlay' : ''} ${state.view === 'apps' ? 'view-apps' : ''}`;
  renderRail();
  if (state.view === 'apps') renderApps();
  else { renderList(); renderThreadPane(false); }
  renderFeed();
}

function buildLayout(app) {
  app.replaceChildren(
    h('aside', { class: 'rail', 'aria-label': 'Phone' }),
    h('section', { class: 'list', 'aria-label': 'Conversations' },
      h('div', { class: 'list-head' },
        h('div', { class: 'row' },
          h('h1', { text: 'Messages' }),
          h('button', { class: 'btn ghost icon-only', title: 'New message (Ctrl+N)', 'aria-label': 'New message', onclick: () => openNew() }, icon('plus'))),
        h('label', { class: 'search' }, icon('search', 'sm'),
          h('input', { id: 'filter', type: 'search', placeholder: 'Search conversations', 'aria-label': 'Search conversations',
            oninput: (e) => { state.filter = e.target.value; renderList(); },
            onkeydown: (e) => { if (e.key === 'ArrowDown') { e.preventDefault(); $('.thread-item')?.focus(); } } }))),
      h('div', { class: 'threads', role: 'listbox', 'aria-label': 'Conversations', onkeydown: listKeys })),
    h('main', { class: 'thread' }),
    h('main', { class: 'apps-page', 'aria-label': 'Phone apps' }),
    h('aside', { class: 'feed', 'aria-label': 'Notifications' }),
  );
}

function renderRail() {
  const d = device();
  const rail = $('.rail');
  const unread = heads().filter(isUnread).length;
  const notes = Object.keys(state.notifications).length;
  swap(rail, () => [
    h('div', { class: 'phone-card' },
      h('div', { class: 'name' }, icon('phone'), h('span', { text: d ? d.name : 'Phone' })),
      d && d.connected ? h('div', { class: 'stats' },
        state.battery && h('span', { title: 'Battery' }, batteryIcon(state.battery.charge, state.battery.charging), `${state.battery.charge}%`),
        ...Object.values(state.signal).slice(0, 1).map((s) => h('span', { title: 'Signal' }, signalIcon(s.strength), s.network_type))) : null,
      h('div', { class: 'conn' },
        h('span', { class: `dot ${d && d.connected ? 'on' : 'wait'}` }),
        d && d.connected ? `Connected${d.address ? ' over Wi-Fi' : ''}` : 'Out of reach. Reconnecting'),
      h('div', { class: 'card-actions' },
        h('button', { class: 'btn', disabled: !(d && d.connected), onclick: ringPhone, title: 'Ring the phone at full volume' }, icon('ring', 'sm'), 'Ring'),
        h('button', { class: 'btn', disabled: !(d && d.connected), onclick: () => quick('ping', 'Ping sent'), title: 'Send a ping' }, icon('bolt', 'sm'), 'Ping'))),
    state.call && state.call.state === 'ringing' ? h('div', { class: 'call' },
      icon('call', 'pulse'),
      h('div', {}, h('b', { text: state.call.name || prettyNumber(state.call.number || '') || 'Unknown caller' }), h('span', { text: 'Incoming call' })),
      h('button', { class: 'btn', onclick: () => quick('mute', 'Ringer muted') }, icon('mute', 'sm'), 'Mute')) : null,
    mediaCard(),
    h('nav', { class: 'nav' },
      h('button', { 'aria-current': state.view === 'messages' ? 'page' : null, onclick: () => showView('messages') }, icon('message'), h('span', { class: 'label', text: 'Messages' }),
        unread ? h('span', { class: 'count label', text: String(unread), 'aria-label': `${unread} unread` }) : null),
      h('button', { 'aria-current': state.view === 'apps' ? 'page' : null, onclick: () => showView('apps') }, icon('apps'), h('span', { class: 'label', text: 'Apps' })),
      h('button', { onclick: toggleFeed }, icon('bell'), h('span', { class: 'label', text: 'Notifications' }),
        notes ? h('span', { class: 'count label', text: String(notes) }) : null)),
    h('div', { class: 'spacer' }),
    updateCard(),
    h('div', { class: 'hint' }, icon('upload', 'sm'), 'Drop files anywhere to send'),
    h('div', { class: 'hint' }, h('kbd', { text: 'Ctrl' }), h('kbd', { text: 'K' }), 'Commands'),
  ]);
}

// ---------- updates ----------
async function loadUpdate(check = false) {
  try { state.update = await api(check ? 'update_check' : 'update_status'); } catch { return; }
  render();
}

async function installUpdate() {
  state.update = { ...state.update, phase: 'downloading', error: null };
  render();
  try { state.update = await api('update_install'); } catch (e) { state.update = { ...state.update, phase: 'idle', error: String(e) }; }
  render();
}

async function restartTether() {
  try { await T.core.invoke('relaunch'); } catch (e) { toast(String(e), 'error'); }
}

async function checkForUpdates() {
  toast('Checking for updates', 'progress', 3000);
  await loadUpdate(true);
  const u = state.update;
  if (!u) return toast("Couldn't check for updates", 'error');
  if (u.error) return toast(u.error, 'error');
  if (u.noReleases) return toast("Couldn't see Tether's releases online", 'error');
  if (!u.latest) toast(`Tether ${u.current} is up to date`);
}

function updateCard() {
  const u = state.update;
  if (!u || u.kind === 'dev' || u.kind === 'store') return null;
  const card = (...kids) => h('div', { class: 'update', role: 'status' }, ...kids);
  const v = u.latest;
  if (u.phase === 'ready') {
    return card(h('b', { text: `Tether ${v} is installed` }), h('span', { text: 'Restart to start using it.' }),
      h('button', { class: 'btn primary', onclick: restartTether }, icon('refresh', 'sm'), 'Restart Tether'));
  }
  if (u.phase === 'installer') {
    return card(h('b', { text: `Tether ${v} is in your Downloads` }),
      h('span', { text: "If the installer didn't open, install it from a terminal:" }),
      h('code', { text: u.installCommand }),
      h('button', { class: 'btn', onclick: () => copy(u.installCommand, 'Command copied') }, icon('copy', 'sm'), 'Copy command'));
  }
  if (!v) return null;
  if (u.phase === 'downloading') {
    return card(h('div', { class: 'row' }, h('span', { class: 'spinner sm', 'aria-hidden': 'true' }), h('b', { text: `Downloading Tether ${v}` })));
  }
  const err = u.error ? h('p', { class: 'note error', role: 'alert' }, icon('alert', 'sm'), u.error) : null;
  if (u.kind === 'other') {
    return card(h('b', { text: `Tether ${v} is available` }), h('span', { text: 'Download it from the releases page.' }), err,
      h('button', { class: 'btn', onclick: () => copy(u.page, 'Link copied') }, icon('copy', 'sm'), 'Copy link'));
  }
  return card(h('b', { text: `Tether ${v} is available` }), err,
    h('button', { class: 'btn primary', onclick: installUpdate }, icon('download', 'sm'), u.error ? 'Try again' : 'Update'));
}

// The daemon checks once a day; pick up what it found.
setInterval(() => { if (state.daemon) loadUpdate(); }, 30 * 60 * 1000);

function renderList() {
  const list = $('.threads');
  const q = state.filter.trim().toLowerCase();
  const items = heads().filter((m) => !q || threadTitle(m.addresses).toLowerCase().includes(q) || (m.body || '').toLowerCase().includes(q));
  swap(list, () => {
    if (!state.threads.size) {
      const d = device();
      return h('div', { class: 'empty' },
        d && d.connected ? h('div', { class: 'spinner' }) : icon('message'),
        h('p', { text: d && d.connected ? 'Loading conversations' : 'Conversations appear when your phone is in reach.' }));
    }
    if (!items.length) return h('div', { class: 'empty' }, h('p', { text: `Nothing matches "${state.filter}".` }));
    return items.map((m) => {
      const title = threadTitle(m.addresses);
      const unread = isUnread(m);
      const preview = (m.type === 2 ? 'You: ' : '') + (m.body || (m.attachments?.length ? 'Picture' : ''));
      return h('button', {
        class: `thread-item ${unread ? 'unread' : ''}`, role: 'option', 'data-key': `t${m.thread_id}`,
        'aria-selected': state.selected === m.thread_id ? 'true' : 'false', onclick: () => selectThread(m.thread_id),
      },
      avatar(title, m.addresses.length > 1),
      h('div', { style: 'min-width:0' }, h('div', { class: 'who', text: title }), h('div', { class: 'preview', text: preview })),
      h('div', { class: 'meta' }, h('time', { text: shortTime(m.date) }), unread ? h('span', { class: 'unread-dot', 'aria-label': 'Unread' }) : null));
    });
  });
}

function listKeys(e) {
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
  const items = [...document.querySelectorAll('.thread-item')];
  const i = items.indexOf(document.activeElement);
  const next = items[e.key === 'ArrowDown' ? Math.min(items.length - 1, i + 1) : Math.max(0, i - 1)];
  if (next) { e.preventDefault(); next.focus(); next.click(); }
}

function selectThread(id) {
  if (state.view !== 'messages') showView('messages');
  state.selected = id;
  const msgs = threadMessages(id);
  if (msgs.length) state.seen[id] = msgs[msgs.length - 1].date;
  saveSeen();
  state.loadingThread = id;
  api('request_thread', { device: state.deviceId, thread_id: id, count: 60 }).catch(() => {});
  setTimeout(() => { if (state.loadingThread === id) { state.loadingThread = null; renderThreadPane(false); } }, 5000);
  renderList();
  renderRail();
  renderThreadPane(true);
}

function renderThreadPane(rebuild) {
  const pane = $('.thread');
  if (!pane) return;
  const id = state.selected;
  if (rebuild || pane.dataset.thread !== String(id)) {
    pane.dataset.thread = String(id);
    pane.replaceChildren(h('div', { class: 'banners' }), h('div', { class: 'thread-head' }), h('div', { class: 'messages', 'aria-live': 'polite' }), id !== null ? buildComposer() : '');
  }
  renderBanners();
  const head = $('.thread-head', pane);
  const box = $('.messages', pane);
  if (id === null) {
    head.hidden = true;
    swap(box, () => h('div', { class: 'empty' }, icon('message'), h('h2', { text: 'Pick a conversation' }),
      h('p', { text: 'Or start a new one. Everything you send goes out from your phone, with your number.' }),
      h('button', { class: 'btn primary', onclick: () => openNew() }, icon('plus', 'sm'), 'New message')));
    return;
  }
  head.hidden = false;
  if (id === 'new') return renderNew(head, box);
  const msgs = threadMessages(id);
  const addrs = msgs[0]?.addresses || [];
  const title = threadTitle(addrs);
  swap(head, () => [
    avatar(title, addrs.length > 1),
    h('div', { class: 'title' }, h('b', { text: title }), subtitle(title, addrs)),
    h('div', { class: 'actions' },
      h('button', { class: 'btn ghost icon-only feed-toggle', title: 'Notifications', 'aria-label': 'Notifications', onclick: toggleFeed }, icon('bell'))),
  ]);
  const atBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 40;
  swap(box, () => messageNodes(id, msgs));
  if (rebuild || atBottom) box.scrollTop = box.scrollHeight;
}

function subtitle(title, addrs) {
  const text = addrs.length > 1 ? addrs.map((a) => nameFor(a.address)).join(', ') : prettyNumber(addrs[0]?.address || '');
  // Short codes and unknown numbers already are the title; don't say it twice.
  return text && text !== title ? h('span', { text }) : null;
}

function messageNodes(id, msgs) {
  const nodes = [];
  if (state.loadingThread === id && msgs.length < 3) nodes.push(h('div', { class: 'day' }, 'Loading earlier messages'));
  let prev = null;
  const group = (msgs[0]?.addresses || []).length > 1;
  const all = [...msgs, ...state.pending.filter((p) => p.thread === id && p.device === state.deviceId).map((p) => ({ ...p, type: 2, _pending: true }))];
  all.forEach((m, i) => {
    if (!prev || !sameDay(new Date(prev.date), new Date(m.date))) nodes.push(h('div', { class: 'day', text: dayLabel(m.date) }));
    const out = m.type === 2;
    const first = !prev || prev.type !== m.type || m.date - prev.date > 10 * 60e3 || (group && prev.addresses?.[0]?.address !== m.addresses?.[0]?.address);
    const next = all[i + 1];
    const last = !next || next.type !== m.type || next.date - m.date > 10 * 60e3;
    const row = h('div', { class: `bubble-row ${out ? 'out' : 'in'} ${first ? 'first' : ''} ${m._pending ? m.state : ''}` });
    if (group && !out && first) row.append(h('div', { class: 'sender', text: nameFor(m.addresses?.[0]?.address) }));
    if (m.body || !m.attachments?.length) row.append(h('div', { class: 'bubble', text: m.body || '' }));
    if (m.attachments?.length) row.append(h('div', { class: 'attachments' }, m.attachments.map(attachmentThumb)));
    const chips = [];
    const code = !out && findCode(m.body);
    if (code) chips.push(h('button', { class: 'chip code', onclick: () => copy(code, 'Code copied') }, icon('key', 'sm'), `Copy ${code}`));
    for (const link of findLinks(m.body).slice(0, 2)) chips.push(h('button', { class: 'chip', onclick: () => copy(link, 'Link copied') }, icon('link', 'sm'), 'Copy link'));
    if (chips.length) row.append(h('div', { class: 'chips' }, chips));
    if (m._pending) {
      row.append(h('div', { class: 'stamp' }, m.state === 'failed'
        ? ['Not sent. ', h('button', { class: 'link-retry', onclick: () => retry(m) }, 'Try again')]
        : m.state === 'unconfirmed'
          ? ['Your phone hasn\'t confirmed this yet. ', h('button', { class: 'link-retry', onclick: () => retry(m) }, 'Send again')]
          : 'Sending'));
    } else if (last) {
      row.append(h('div', { class: 'stamp', text: new Date(m.date).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }) }));
    }
    nodes.push(row);
    prev = m;
  });
  return nodes;
}

function buildComposer() {
  const d = device();
  const ta = h('textarea', { rows: 1, 'data-key': 'composer', placeholder: d && d.connected ? 'Text message' : 'Your phone is out of reach', 'aria-label': 'Message',
    disabled: !(d && d.connected),
    oninput: () => { ta.style.height = 'auto'; ta.style.height = Math.min(140, ta.scrollHeight) + 'px'; },
    onkeydown: (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(ta); } } });
  return h('div', { class: 'composer' },
    h('div', { class: 'composer-box' }, ta,
      h('button', { class: 'btn primary icon-only', 'aria-label': 'Send', title: 'Send (Enter)', onclick: () => send(ta), disabled: !(d && d.connected) }, icon('send'))),
    h('div', { class: 'note' }, h('span', { text: 'Sent as a text from your phone' }), h('span', { text: 'Shift+Enter for a new line' })));
}

async function send(ta) {
  const body = ta.value.trim();
  if (!body) return;
  let addresses, thread = state.selected;
  if (thread === 'new') {
    addresses = state.newTo;
    if (!addresses.length) return toast('Add who the message is for first', 'error');
    thread = null;
  } else {
    addresses = (threadMessages(thread)[0]?.addresses || []).map((a) => a.address);
  }
  ta.value = '';
  ta.style.height = 'auto';
  const p = { key: `p${Date.now()}-${Math.random()}`, device: state.deviceId, body, thread, addresses: addresses.map((a) => ({ address: a })), date: Date.now(), state: 'pending' };
  state.pending.push(p);
  if (thread !== null) renderThreadPane(false);
  else toast('Sending', 'progress', 4000);
  try {
    await api('sms_send', { device: state.deviceId, addresses, body });
  } catch (e) {
    p.state = 'failed';
    renderThreadPane(false);
    return toast(String(e), 'error');
  }
  // No echo within 45 s: maybe no signal or permission, maybe just slow. Say so, don't claim failure.
  setTimeout(() => { if (state.pending.includes(p)) { p.state = 'unconfirmed'; renderThreadPane(false); } }, 45000);
}

function retry(m) {
  // Rendered messages are copies, so find the pending entry by its key.
  const i = state.pending.findIndex((p) => p.key === m.key);
  if (i >= 0) state.pending.splice(i, 1);
  const ta = $('.composer textarea');
  if (ta) { ta.value = p.body; send(ta); }
}

// ---------- new message ----------
function openNew(contact) {
  if (state.view !== 'messages') showView('messages');
  state.selected = 'new';
  state.newTo = contact ? [contact.number] : [];
  renderList();
  renderThreadPane(true);
  setTimeout(() => (contact ? $('.composer textarea') : $('#to'))?.focus(), 0);
}

function renderNew(head, box) {
  swap(head, () => [
    h('div', { class: 'title' }, h('b', { text: 'New message' }),
      h('span', { text: state.newTo.length ? state.newTo.map(nameFor).join(', ') : 'Choose a contact or type a number' })),
  ]);
  const q = ($('#to')?.value || '').trim().toLowerCase();
  const matches = state.contacts.filter((c) => !q || c.name.toLowerCase().includes(q) || c.number.includes(digits(q) || '\u0000')).slice(0, 8);
  swap(box, () => h('div', { style: 'display:grid;gap:10px;max-width:460px' },
    h('label', { class: 'search' }, h('span', { text: 'To' }),
      h('input', { id: 'to', 'data-key': 'to', placeholder: 'Name or number', autocomplete: 'off',
        oninput: () => renderNew(head, box),
        onkeydown: (e) => {
          if (e.key !== 'Enter') return;
          const v = e.target.value.trim();
          if (matches[0] && q) addRecipient(matches[0].number);
          else if (digits(v).length >= 3) addRecipient(v);
        } })),
    state.newTo.length ? h('div', { class: 'chips' }, state.newTo.map((n) => h('button', { class: 'chip', onclick: () => { state.newTo = state.newTo.filter((x) => x !== n); renderNew(head, box); }, title: 'Remove' }, nameFor(n), icon('x', 'sm')))) : null,
    !state.contacts.length ? h('p', { class: 'hint', text: 'Contacts load from your phone once it is connected. You can type a number meanwhile.' }) : null,
    ...matches.map((c) => h('button', { class: 'thread-item', onclick: () => addRecipient(c.number) },
      avatar(c.name, false, 'sm'), h('div', {}, h('div', { class: 'who', text: c.name }), h('div', { class: 'preview', text: prettyNumber(c.number) })), '')),
  ));
}

function addRecipient(n) {
  if (!state.newTo.includes(n)) state.newTo.push(n);
  const to = $('#to'); if (to) to.value = '';
  renderThreadPane(false);
  $('.composer textarea')?.focus();
}

// ---------- media ----------
function activePlayer() {
  const ps = Object.values(state.media || {}).filter((p) => p.title);
  return ps.find((p) => p.is_playing) || ps[0] || null;
}

// The phone reports position only when something changes, so count forward while playing.
function mediaPosition(p) {
  if (!p.position_at) return p.position_ms;
  const pos = p.position_ms + (p.is_playing ? Date.now() - p.position_at : 0);
  return p.length_ms > 0 ? Math.min(pos, p.length_ms) : pos;
}

function clock(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h_ = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = String(s % 60).padStart(2, '0');
  return h_ ? `${h_}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`;
}

function mediaAction(p, action) {
  api('media_action', { device: state.deviceId, player: p.name, action }).catch((e) => toast(String(e), 'error'));
}

function mediaCard() {
  const d = device();
  const p = activePlayer();
  if (!p || !(d && d.connected)) return null;
  const pos = mediaPosition(p);
  const by = p.artist || p.album || '';
  return h('section', { class: 'media', 'aria-label': 'Playing on your phone' },
    h('div', { class: 'media-top' },
      p.art_path ? h('img', { class: 'art', src: T.core.convertFileSrc(p.art_path), alt: '' })
        : h('div', { class: 'art placeholder' }, icon('music')),
      h('div', { class: 'meta', title: `${p.title}${by ? ` by ${by}` : ''}, on ${p.name}` }, h('b', { text: p.title }), h('span', { text: by }))),
    p.length_ms > 0 ? h('div', { class: 'progress' },
      h('input', {
        type: 'range', min: '0', max: String(p.length_ms), step: '1000', value: String(pos),
        disabled: !p.can_seek, 'aria-label': 'Position in the song', 'aria-valuetext': `${clock(pos)} of ${clock(p.length_ms)}`,
        oninput: (e) => { const t = $('.media .pos'); if (t) t.textContent = clock(+e.target.value); },
        onchange: (e) => api('media_seek', { device: state.deviceId, player: p.name, position_ms: +e.target.value }).catch((err) => toast(String(err), 'error')),
      }),
      h('div', { class: 'times' }, h('span', { class: 'pos', text: clock(pos) }), h('span', { text: clock(p.length_ms) }))) : null,
    h('div', { class: 'media-controls' },
      h('button', { class: 'btn ghost icon-only', disabled: !p.can_go_previous, title: 'Previous', 'aria-label': 'Previous', onclick: () => mediaAction(p, 'Previous') }, icon('prev')),
      h('button', { class: 'btn primary icon-only play', disabled: !(p.is_playing ? p.can_pause : p.can_play), title: p.is_playing ? 'Pause' : 'Play', 'aria-label': p.is_playing ? 'Pause' : 'Play', onclick: () => mediaAction(p, 'PlayPause') }, icon(p.is_playing ? 'pause' : 'play')),
      h('button', { class: 'btn ghost icon-only', disabled: !p.can_go_next, title: 'Next', 'aria-label': 'Next', onclick: () => mediaAction(p, 'Next') }, icon('next'))));
}

// Move the position along without rebuilding the card (which would interrupt a drag).
setInterval(() => {
  const p = activePlayer();
  const range = $('.media input[type=range]');
  if (!p || !p.is_playing || !range || document.activeElement === range) return;
  const pos = mediaPosition(p);
  range.value = String(pos);
  range.setAttribute('aria-valuetext', `${clock(pos)} of ${clock(p.length_ms)}`);
  const t = $('.media .pos');
  if (t) t.textContent = clock(pos);
}, 1000);

// ---------- picture messages ----------
const waitingAttachments = new Map(); // unique_identifier -> resolve(path)

function isVisual(a) { return /^(image|video)\//.test(a.mime_type || ''); }

function attachmentThumb(a) {
  const video = (a.mime_type || '').startsWith('video/');
  // The phone sends a small JPEG inline; only base64 characters may reach the data URL.
  const thumb = a.encoded_thumbnail && /^[A-Za-z0-9+/=\s]+$/.test(a.encoded_thumbnail) ? `data:image/jpeg;base64,${a.encoded_thumbnail}` : null;
  if (!isVisual(a)) {
    return h('button', { class: 'attach file', onclick: () => openAttachment(a) }, icon('download', 'sm'), 'Attachment');
  }
  return h('button', { class: 'attach', title: video ? 'Open the video' : 'Open the picture', 'aria-label': video ? 'Open the video' : 'Open the picture', onclick: () => openAttachment(a) },
    thumb ? h('img', { src: thumb, alt: '' }) : icon('image'),
    video ? h('span', { class: 'badge' }, icon('play', 'sm')) : null);
}

async function fetchAttachment(a) {
  const r = await api('sms_attachment', { device: state.deviceId, part_id: a.part_id, unique_identifier: a.unique_identifier });
  if (r && r.path) return r.path;
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => { waitingAttachments.delete(a.unique_identifier); reject(new Error('Your phone didn\'t send it. Try again with the phone unlocked.')); }, 60000);
    waitingAttachments.set(a.unique_identifier, { resolve: (p) => { clearTimeout(t); resolve(p); }, reject: (e) => { clearTimeout(t); reject(e); } });
  });
}

function openAttachment(a) {
  const root = $('#palette-root');
  const back = document.activeElement;
  const close = () => { root.replaceChildren(); back?.focus?.(); };
  const body = h('div', { class: 'viewer-body' }, h('div', { class: 'spinner' }), h('p', { text: 'Getting it from your phone' }));
  const save = h('button', { class: 'btn', disabled: true, onclick: async () => {
    try { const r = await api('sms_attachment_save', { device: state.deviceId, unique_identifier: a.unique_identifier }); toast(`Saved to ${r.path.split('/').slice(-2).join('/')}`); } catch (e) { toast(String(e), 'error'); }
  } }, icon('download', 'sm'), 'Save to Downloads');
  const closeBtn = h('button', { class: 'btn ghost icon-only', title: 'Close (Esc)', 'aria-label': 'Close', onclick: close }, icon('x'));
  root.replaceChildren(h('div', { class: 'palette-backdrop viewer-backdrop', onclick: (e) => { if (e.target === e.currentTarget) close(); },
    onkeydown: (e) => { if (e.key === 'Escape') close(); } },
  h('div', { class: 'viewer', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Picture message' },
    h('div', { class: 'viewer-bar' }, save, closeBtn), body)));
  closeBtn.focus();
  fetchAttachment(a).then((path) => {
    const src = T.core.convertFileSrc(path);
    const kind = a.mime_type || '';
    body.replaceChildren(kind.startsWith('video/') ? h('video', { src, controls: true, autoplay: true })
      : kind.startsWith('image/') ? h('img', { src, alt: 'Picture from the conversation' })
        : h('p', { text: 'Downloaded. Save it to open it with another app.' }));
    save.disabled = false;
  }).catch((e) => {
    body.replaceChildren(icon('alert', 'lg'), h('p', { text: e.message || String(e) }),
      h('button', { class: 'btn', onclick: () => { close(); openAttachment(a); } }, icon('refresh', 'sm'), 'Try again'));
  });
}

// ---------- banners ----------
// Another KDE Connect app on this computer holds the discovery port, so the phone may talk to it.
function portBanner() {
  if (!state.me || state.me.discoveryListening !== false) return null;
  return h('div', { class: 'banner warn' }, icon('alert', 'sm'),
    'KDE Connect or Valent is also running on this computer, so your phone may connect to it instead. Quit it and Tether takes over within a few seconds.');
}

function renderBanners() {
  const slot = $('.thread .banners');
  if (!slot) return;
  const d = device();
  const asking = state.devices.filter((x) => x.pair === 'requested_by_peer');
  swap(slot, () => [
    portBanner(),
    d && !d.connected ? h('div', { class: 'banner warn' }, icon('wifi', 'sm'),
      `${d.name} is out of reach. Tether keeps trying; check the phone is on the same Wi-Fi.`,
      h('button', { class: 'btn', onclick: () => quick('refresh', 'Looking for your phone', true) }, icon('refresh', 'sm'), 'Look now')) : null,
    ...asking.map((x) => h('div', { class: 'banner' }, icon('phone', 'sm'),
      h('span', {}, `${x.name} wants to pair. Code `, h('b', { text: x.verification_key || '' })),
      h('button', { class: 'btn primary', onclick: () => pairAction('accept', x.id) }, 'Accept'),
      h('button', { class: 'btn', onclick: () => pairAction('reject', x.id) }, 'Decline'))),
  ]);
}

// ---------- phone apps (each in its own window, over wireless debugging + scrcpy) ----------
const apps = {
  status: null,
  list: null, // [{name, package, system}]
  loading: false,
  error: null,
  filter: '',
  opening: null, // package being opened
  setup: false, // "Set up again" pressed
  byCode: false,
  manual: false, // "Enter the address yourself" opened
  address: '', // typed by hand, only when Tether can't find the phone
  code: '',
  details: null, // diagnostics text shown under an error, or 'loading'
  formError: null,
  timer: null,
  polling: false,
  serial: null, // adb device the list belongs to
  // App name (lower case) -> icon file, learned from phone notifications, kept between runs.
  icons: (() => { try { return JSON.parse(localStorage.getItem('tether.appIcons') || '{}'); } catch { return {}; } })(),
  menu: new Set(), // packages with an entry in the desktop's app menu
  recent: (() => { try { return JSON.parse(localStorage.getItem('tether.recentApps') || '[]'); } catch { return []; } })(),
};

function showView(view) {
  state.view = view;
  clearTimeout(apps.timer);
  if (view === 'apps') pollApps();
  render();
  if (view === 'messages') $('#filter')?.focus();
}

async function pollApps() {
  clearTimeout(apps.timer);
  // One poll at a time: several buttons call this, and two loops would each keep a timer.
  if (state.view !== 'apps' || apps.polling) return;
  apps.polling = true;
  try {
    apps.status = await api('apps_status');
  } catch (e) {
    apps.status = { failed: String(e) };
  }
  apps.polling = false;
  if (state.view !== 'apps') return;
  const st = apps.status;
  if (st.device) {
    apps.setup = false;
    if (st.device.serial !== apps.serial) {
      // A different phone: its apps, not the last one's.
      apps.serial = st.device.serial;
      apps.list = null;
      apps.error = null;
    }
    if (!apps.list && !apps.loading && !apps.error) loadApps();
  } else if (st.adb && st.scrcpy && !st.scrcpyTooOld && st.phase === 'idle' && !st.error && wantsSetup()) {
    // The setup screen shows the QR code straight away; nothing to click first.
    api('apps_pair_qr').then(pollApps).catch(() => {});
  }
  renderApps();
  const busy = st.phase && st.phase !== 'idle';
  clearTimeout(apps.timer);
  apps.timer = setTimeout(pollApps, busy || !st.device ? 1500 : 10000);
}

function wantsSetup() {
  return !apps.byCode && (!apps.status.pairedBefore || apps.setup);
}

async function loadApps() {
  apps.loading = true;
  apps.error = null;
  renderApps();
  try {
    apps.list = await api('apps_list');
    apps.menu = new Set(await api('apps_menu').catch(() => []));
  } catch (e) { apps.error = String(e); }
  apps.loading = false;
  renderApps();
}

async function openApp(app) {
  if (apps.opening) return;
  apps.opening = app ? app.package : 'screen';
  renderApps();
  toast(app ? `Opening ${app.name}` : 'Opening your phone screen', 'progress', 4000);
  try {
    await api('apps_open', app ? { package: app.package, title: app.name } : { title: 'Phone screen' });
    if (app) {
      apps.recent = [app.package, ...apps.recent.filter((p) => p !== app.package)].slice(0, 6);
      try { localStorage.setItem('tether.recentApps', JSON.stringify(apps.recent)); } catch { /* private mode */ }
    }
  } catch (e) {
    toast(String(e), 'error', 5000);
  }
  apps.opening = null;
  renderApps();
}

function renderApps() {
  const page = $('.apps-page');
  if (!page || state.view !== 'apps') return;
  const st = apps.status;
  swap(page, () => {
    if (!st) return h('div', { class: 'empty' }, h('div', { class: 'spinner' }), h('p', { text: 'Checking your phone' }));
    if (st.failed) return h('div', { class: 'empty' }, icon('alert'), h('p', { text: st.failed }),
      h('button', { class: 'btn', 'data-key': 'apps-retry', onclick: pollApps }, icon('refresh', 'sm'), 'Try again'));
    if (!st.adb || !st.scrcpy || st.scrcpyTooOld) return appsHelper(st);
    if (!st.device) return appsSetup(st);
    return appsGrid(st);
  });
}

function appsIntro(title, text) {
  return h('div', { class: 'apps-intro' }, h('h1', { text: title }), h('p', { text }));
}

function appsHelper(st) {
  const downloading = st.phase === 'downloading';
  return h('div', { class: 'apps-setup narrow' },
    appsIntro("Use your phone's apps on this computer",
      'Each app opens in its own window. This needs scrcpy, a free open-source helper.'),
    st.scrcpyTooOld ? h('p', { class: 'note', text: 'The scrcpy installed here is too old for app windows. Version 3.0 or newer is needed.' }) : null,
    st.error ? h('p', { class: 'note error', role: 'alert' }, icon('alert', 'sm'), st.error) : null,
    st.canDownload
      ? h('div', { class: 'row' },
        h('button', { class: 'btn primary', 'data-key': 'apps-download', disabled: downloading, onclick: () => api('apps_download').then(pollApps).catch((e) => toast(String(e), 'error')) },
          downloading ? h('span', { class: 'spinner sm', 'aria-hidden': 'true' }) : icon('download', 'sm'),
          downloading ? 'Downloading (18 MB)' : 'Download helper (18 MB)'),
        h('span', { class: 'hint', text: 'Or install scrcpy 3.0 or newer yourself.' }))
      : h('div', { class: 'row' },
        h('button', { class: 'btn', 'data-key': 'apps-check', onclick: pollApps }, icon('refresh', 'sm'), 'Check again'),
        h('span', { class: 'hint', text: 'Install scrcpy 3.0 or newer and adb first.' })));
}

function appsSetup(st) {
  if (st.pairedBefore && !apps.setup && !apps.byCode) {
    return h('div', { class: 'apps-setup narrow' },
      appsIntro('Reconnecting to your phone', "Your phone reconnects by itself when Wireless debugging is on and it's on the same Wi-Fi as this computer."),
      h('ol', { class: 'steps' }, h('li', { text: 'On your phone, open Settings, then Developer options.' }), h('li', { text: 'Turn on Wireless debugging.' })),
      st.error ? h('div', { class: 'stack' }, h('p', { class: 'note error', role: 'alert' }, icon('alert', 'sm'), st.error), appsDetails())
        : h('div', { class: 'row' }, h('span', { class: 'spinner sm', 'aria-hidden': 'true' }), h('span', { class: 'hint', text: 'Looking for your phone' })),
      h('div', { class: 'row' }, h('button', { class: 'btn', 'data-key': 'apps-setup-again', onclick: () => { apps.setup = true; api('apps_cancel').then(pollApps); } }, 'Set up again')));
  }
  const phase = st.phase;
  let side;
  if (apps.byCode) side = codeForm(st);
  else if (phase === 'waiting' && st.qr) {
    side = h('div', { class: 'qr-card' },
      h('img', { class: 'qr', alt: 'QR code to scan with your phone', src: `data:image/svg+xml;utf8,${encodeURIComponent(st.qr)}` }),
      h('div', { class: 'row' }, h('span', { class: 'spinner sm', 'aria-hidden': 'true' }), h('span', { text: 'Waiting for your phone' })));
  } else if (phase === 'pairing' || phase === 'connecting') {
    side = h('div', { class: 'qr-card' }, h('div', { class: 'spinner' }), h('p', { text: phase === 'pairing' ? 'Pairing with your phone' : 'Connecting to your phone' }));
  } else {
    side = h('div', { class: 'qr-card' },
      st.error ? h('p', { class: 'note error', role: 'alert' }, icon('alert', 'sm'), st.error) : h('div', { class: 'spinner' }),
      st.error ? appsDetails() : null,
      st.error ? h('button', { class: 'btn primary', 'data-key': 'apps-new-code', onclick: () => { apps.details = null; api('apps_pair_qr').then(pollApps); } }, icon('refresh', 'sm'), 'Show a new code') : null);
  }
  return h('div', { class: 'apps-setup' },
    appsIntro("Use your phone's apps on this computer", 'Each app opens in its own window, with its sound on Android 11 or newer. Set it up once; it takes a minute.'),
    h('div', { class: 'setup-cols' },
      h('ol', { class: 'steps' },
        h('li', {}, h('b', { text: 'Turn on Developer options. ' }), 'On your phone, open Settings, About phone, Software information, and tap Build number seven times.'),
        h('li', {}, h('b', { text: 'Turn on Wireless debugging. ' }), 'Open Settings, Developer options, Wireless debugging. Your phone must be on the same Wi-Fi as this computer.'),
        apps.byCode
          ? h('li', {}, h('b', { text: 'Tap Pair device with pairing code ' }), 'and type the 6-digit code here.')
          : h('li', {}, h('b', { text: 'Tap Pair device with QR code ' }), 'and scan the code here.')),
      side),
    h('button', { class: 'btn ghost link-btn', 'data-key': 'apps-mode', onclick: () => { apps.byCode = !apps.byCode; apps.formError = null; apps.details = null; api(apps.byCode ? 'apps_find_port' : 'apps_pair_qr').then(pollApps); } },
      apps.byCode ? 'Scan a QR code instead' : 'Pair with a code instead'));
}

function codeForm(st) {
  const busy = st.phase === 'pairing' || st.phase === 'connecting';
  const submit = async (e) => {
    e.preventDefault();
    apps.formError = null;
    apps.details = null;
    const params = { code: apps.code };
    if (apps.manual && apps.address.trim()) params.address = apps.address.trim();
    renderApps();
    try {
      await api('apps_pair_code', params);
    } catch (err) {
      apps.formError = String(err);
      // A failed try ends the watch for the pairing port; start it again for the next try.
      api('apps_find_port').catch(() => {});
    }
    pollApps();
  };
  const field = (label, hint, attrs) => h('label', {},
    h('span', { text: label }),
    h('input', { autocomplete: 'off', spellcheck: 'false', ...attrs }),
    hint ? h('span', { class: 'hint', text: hint }) : null);
  return h('form', { class: 'qr-card code-form', onsubmit: submit },
    field('Wi-Fi pairing code', '6 digits, shown on your phone.', {
      class: 'big', 'data-key': 'apps-code', inputmode: 'numeric', placeholder: '123456', required: true, maxlength: 7, value: apps.code,
      oninput: (e) => { apps.code = e.target.value; },
    }),
    apps.manual ? field('IP address and port', 'Shown on the same phone screen, like 192.168.1.20:41234.', {
      'data-key': 'apps-address', inputmode: 'decimal', placeholder: '192.168.1.20:41234', value: apps.address,
      oninput: (e) => { apps.address = e.target.value; },
    }) : null,
    apps.formError ? h('p', { class: 'note error', role: 'alert' }, icon('alert', 'sm'), apps.formError) : null,
    apps.formError ? appsDetails() : null,
    h('button', { class: 'btn primary', type: 'submit', 'data-key': 'apps-pair', disabled: busy },
      busy ? h('span', { class: 'spinner sm', 'aria-hidden': 'true' }) : null, busy ? (st.phase === 'pairing' ? 'Pairing' : 'Connecting') : 'Pair'),
    apps.manual ? null : h('button', { type: 'button', class: 'btn ghost link-btn small', 'data-key': 'apps-manual', onclick: () => { apps.manual = true; renderApps(); $('[data-key="apps-address"]')?.focus(); } },
      'Enter the address yourself'));
}

/** What Tether sees while looking for the phone, for when pairing fails. No terminal needed. */
function appsDetails() {
  if (!apps.details) {
    return h('button', { type: 'button', class: 'btn ghost link-btn small', 'data-key': 'apps-details', onclick: async () => {
      apps.details = 'loading';
      renderApps();
      try { apps.details = await api('apps_doctor'); } catch (e) { apps.details = `Couldn't collect details: ${e}`; }
      renderApps();
    } }, 'Show details');
  }
  if (apps.details === 'loading') {
    return h('div', { class: 'row hint' }, h('span', { class: 'spinner sm', 'aria-hidden': 'true' }), h('span', { text: 'Collecting details' }));
  }
  return h('div', { class: 'apps-details' },
    h('pre', { tabindex: '0', 'aria-label': 'Details', text: apps.details }),
    h('button', { type: 'button', class: 'btn small', 'data-key': 'apps-details-copy', onclick: () => copy(apps.details, 'Details copied') }, 'Copy details'));
}

/** Phone notifications carry the app's icon; remember it so the Apps page shows real icons. */
function learnAppIcons() {
  let changed = false;
  for (const n of Object.values(state.notifications)) {
    const key = (n.app || '').toLowerCase();
    if (key && n.icon_path && apps.icons[key] !== n.icon_path) { apps.icons[key] = n.icon_path; changed = true; }
  }
  if (!changed) return;
  try { localStorage.setItem('tether.appIcons', JSON.stringify(apps.icons)); } catch { /* private mode */ }
  if (state.view === 'apps') renderApps();
}

function appIcon(a) {
  const path = apps.icons[a.name.toLowerCase()];
  if (!path) return avatar(a.name, false);
  const img = h('img', { class: 'avatar app-icon', alt: '', src: T.core.convertFileSrc(path) });
  // The icon file may have been pruned; fall back to letters and forget it.
  img.addEventListener('error', () => {
    delete apps.icons[a.name.toLowerCase()];
    img.replaceWith(avatar(a.name, false));
  }, { once: true });
  return img;
}

function appTile(a) {
  const opening = apps.opening === a.package;
  const inMenu = apps.menu.has(a.package);
  return h('div', { class: 'app-tile-wrap' },
    h('button', { class: 'app-tile', 'data-key': `app-${a.package}`, title: a.package, disabled: !!apps.opening && !opening, onclick: () => openApp(a) },
      opening ? h('div', { class: 'avatar' }, h('span', { class: 'spinner sm', 'aria-hidden': 'true' })) : appIcon(a),
      h('span', { class: 'name', text: opening ? 'Opening' : a.name })),
    h('button', { class: `pin ${inMenu ? 'on' : ''}`, 'data-key': `pin-${a.package}`,
      title: inMenu ? `Remove ${a.name} from this computer's app menu` : `Add ${a.name} to this computer's app menu`,
      'aria-label': inMenu ? `Remove ${a.name} from the app menu` : `Add ${a.name} to the app menu`,
      'aria-pressed': inMenu ? 'true' : 'false', onclick: () => toggleMenuEntry(a) }, icon('pin', 'sm')));
}

async function toggleMenuEntry(a) {
  const on = !apps.menu.has(a.package);
  try {
    await api('apps_menu_set', { package: a.package, name: a.name, icon: apps.icons[a.name.toLowerCase()] || null, on });
    if (on) apps.menu.add(a.package); else apps.menu.delete(a.package);
    toast(on ? `${a.name} is in your app menu` : `${a.name} removed from your app menu`);
  } catch (e) {
    toast(String(e), 'error');
  }
  renderApps();
}

function appsGrid(st) {
  const q = apps.filter.trim().toLowerCase();
  const all = (apps.list || []).slice().sort((a, b) => a.name.localeCompare(b.name));
  const shown = all.filter((a) => !q || a.name.toLowerCase().includes(q));
  const recent = q ? [] : apps.recent.map((p) => all.find((a) => a.package === p)).filter(Boolean);
  let body;
  if (apps.loading && !apps.list) body = h('div', { class: 'empty' }, h('div', { class: 'spinner' }), h('p', { text: 'Getting the list of apps from your phone' }));
  else if (apps.error) body = h('div', { class: 'empty' }, icon('alert'), h('p', { text: apps.error }), h('button', { class: 'btn', 'data-key': 'apps-reload', onclick: loadApps }, icon('refresh', 'sm'), 'Try again'));
  else if (!shown.length) body = h('div', { class: 'empty' }, h('p', { text: q ? `No app matches "${apps.filter}".` : 'No apps found on the phone.' }));
  else {
    body = h('div', { class: 'apps-body' },
      recent.length ? [h('h2', { text: 'Recent' }), h('div', { class: 'app-grid' }, recent.map(appTile))] : null,
      recent.length ? h('h2', { text: 'All apps' }) : null,
      h('div', { class: 'app-grid' }, shown.map(appTile)));
  }
  return [
    h('div', { class: 'apps-head' },
      h('div', { class: 'row' },
        h('div', {}, h('h1', { text: 'Apps' }), h('p', { class: 'sub', text: `From ${device()?.name || st.device.model || 'your phone'}. Pin an app to add it to this computer's app menu.` })),
        h('button', { class: 'btn', 'data-key': 'apps-screen', disabled: !!apps.opening, onclick: () => openApp(null) },
          apps.opening === 'screen' ? h('span', { class: 'spinner sm', 'aria-hidden': 'true' }) : icon('phone', 'sm'), 'Phone screen')),
      h('label', { class: 'search' }, icon('search', 'sm'),
        h('input', { type: 'search', 'data-key': 'apps-filter', placeholder: 'Search apps', 'aria-label': 'Search apps', value: apps.filter,
          oninput: (e) => { apps.filter = e.target.value; renderApps(); },
          onkeydown: (e) => { if (e.key === 'Enter' && shown[0]) openApp(shown[0]); } }))),
    body,
  ];
}

// ---------- notifications ----------
function toggleFeed() {
  if (window.innerWidth > 1180) { $('.feed button, .feed input')?.focus(); return; }
  state.feedOverlay = !state.feedOverlay;
  $('#app').classList.toggle('feed-overlay', state.feedOverlay);
}

function recentCode() {
  const cutoff = Date.now() - 10 * 60e3;
  const fromTexts = heads().filter((m) => m.type === 1 && m.date > cutoff).map((m) => ({ code: findCode(m.body), from: threadTitle(m.addresses), at: m.date }));
  const fromNotes = Object.values(state.notifications).filter((n) => n.time > cutoff).map((n) => ({ code: findCode(`${n.title} ${n.text}`), from: n.app, at: n.time }));
  return [...fromTexts, ...fromNotes].filter((c) => c.code).sort((a, b) => b.at - a.at)[0] || null;
}

function renderFeed() {
  learnAppIcons();
  const feed = $('.feed');
  const notes = Object.values(state.notifications).sort((a, b) => b.time - a.time);
  const clearable = notes.filter((n) => n.dismissable);
  const code = recentCode();
  swap(feed, () => [
    h('div', { class: 'feed-head' }, h('h2', { text: 'Notifications' }),
      h('div', {},
        clearable.length ? h('button', { class: 'btn ghost', onclick: () => clearable.forEach((n) => dismiss(n)) }, 'Clear all') : null,
        state.feedOverlay ? h('button', { class: 'btn ghost icon-only', 'aria-label': 'Close', onclick: toggleFeed }, icon('x')) : null)),
    h('div', { class: 'feed-body' },
      code ? h('div', { class: 'code-card' },
        h('div', { class: 'from' }, icon('key', 'sm'), `Code from ${code.from}`),
        h('div', { class: 'row' }, h('span', { class: 'digits', text: code.code }),
          h('button', { class: 'btn primary', onclick: () => copy(code.code, 'Code copied') }, icon('copy', 'sm'), 'Copy'))) : null,
      notes.length ? notes.map(noteCard) : h('div', { class: 'empty' }, icon('bell'), h('p', { text: 'No notifications on your phone.' }))),
  ]);
}

function appBadge(n) {
  if (n.icon_path) return h('img', { src: T.core.convertFileSrc(n.icon_path), alt: '' });
  let hash = 0;
  for (const ch of n.app || '?') hash = (hash * 31 + ch.codePointAt(0)) >>> 0;
  return h('span', { class: 'app-badge', style: `background:hsl(${hash % 360} 45% 46%)`, text: (n.app || '?')[0].toUpperCase() });
}

function noteCard(n) {
  const card = h('article', { class: 'note-card', 'aria-label': `${n.app}: ${n.title}` },
    h('div', { class: 'app-row' }, appBadge(n), h('span', { text: n.app }), h('time', { text: ago(n.time) })),
    n.title ? h('div', { class: 't', text: n.title }) : null,
    n.text ? h('div', { class: 'b', text: n.text }) : null);
  if (n.dismissable) card.append(h('button', { class: 'btn ghost icon-only x', 'aria-label': 'Dismiss', title: 'Dismiss on phone', onclick: () => dismiss(n) }, icon('x', 'sm')));
  const acts = [];
  if (n.reply_id) acts.push(h('button', { class: 'btn', onclick: () => openReply(card, n) }, icon('reply', 'sm'), 'Reply'));
  const code = findCode(`${n.title} ${n.text}`);
  if (code) acts.push(h('button', { class: 'chip code', onclick: () => copy(code, 'Code copied') }, icon('key', 'sm'), `Copy ${code}`));
  for (const a of n.actions || []) acts.push(h('button', { class: 'btn', onclick: () => noteAction(n, a) }, a));
  if (acts.length) card.append(h('div', { class: 'acts' }, acts));
  return card;
}

function openReply(card, n) {
  if ($('.reply-box', card)) return $('.reply-box input', card).focus();
  const input = h('input', { 'data-key': `reply-${n.id}`, placeholder: `Reply in ${n.app}`, 'aria-label': `Reply to ${n.title}`,
    onkeydown: (e) => { if (e.key === 'Enter') go(); if (e.key === 'Escape') box.remove(); } });
  const go = async () => {
    const message = input.value.trim();
    if (!message) return;
    try {
      await api('notification_reply', { device: state.deviceId, reply_id: n.reply_id, message });
      box.remove();
      toast(`Reply handed to ${n.app} on your phone`);
    } catch (e) { toast(String(e), 'error'); }
  };
  const box = h('div', { class: 'reply-box' }, input, h('button', { class: 'btn primary icon-only', 'aria-label': 'Send reply', onclick: go }, icon('send', 'sm')));
  card.append(box);
  input.focus();
}

async function dismiss(n) {
  try { await api('notification_dismiss', { device: state.deviceId, id: n.id }); } catch (e) { toast(String(e), 'error'); }
}
async function noteAction(n, action) {
  try { await api('notification_action', { device: state.deviceId, id: n.id, action }); toast(`"${action}" passed to your phone`); } catch (e) { toast(String(e), 'error'); }
}

// ---------- pairing ----------
function renderPairPage(app) {
  app.className = 'app single';
  const near = state.devices.filter((d) => d.connected || d.pair !== 'unpaired');
  swap(app, () => h('div', { class: 'pair-page' },
    h('header', {}, h('h1', { text: 'Connect your phone' }),
      h('p', { text: 'Tether works with the KDE Connect app on Android. Your texts, notifications and files stay on your own network.' })),
    portBanner(),
    h('ol', { class: 'steps' },
      h('li', { text: 'Install KDE Connect from Google Play or F-Droid.' }),
      h('li', { text: 'Open it while your phone is on the same Wi-Fi as this computer.' }),
      h('li', {}, 'Pick your phone below, or choose ', h('b', { text: state.me ? state.me.deviceName : 'this computer' }), ' in the app.')),
    near.length ? near.map(pairRow) : h('div', { class: 'searching' }, h('div', { class: 'spinner' }), 'Looking for phones on this network'),
    h('div', { class: 'card-actions' },
      h('button', { class: 'btn', onclick: () => quick('refresh', 'Looking for phones', true) }, icon('refresh', 'sm'), 'Look again'),
      h('button', { class: 'btn ghost', onclick: connectByAddress }, 'Phone not showing up?')),
  ));
}

function pairRow(d) {
  const state_ = {
    requested_by_peer: [h('span', { class: 'verify', text: d.verification_key || '' }),
      h('button', { class: 'btn primary', onclick: () => pairAction('accept', d.id) }, 'Accept'),
      h('button', { class: 'btn', onclick: () => pairAction('reject', d.id) }, 'Decline')],
    requested_by_us: [h('span', { class: 'verify', text: d.verification_key || '' }), h('div', { class: 'spinner', title: 'Waiting for the phone' })],
  }[d.pair] || [h('button', { class: 'btn primary', onclick: () => pairAction('pair', d.id) }, 'Pair')];
  const sub = { requested_by_peer: 'Wants to pair. Check the phone shows the same code.', requested_by_us: 'Accept on the phone. It shows the same code.' }[d.pair] || 'Ready to pair';
  return h('div', { class: 'device-row' }, icon('phone'), h('div', { class: 'grow' }, h('b', { text: d.name }), h('span', { text: sub })), ...state_);
}

async function pairAction(action, id) {
  try { await api(action, { device: id }); } catch (e) { toast(String(e), 'error'); }
}

function connectByAddress() {
  const root = $('#palette-root');
  const input = h('input', { placeholder: 'Phone address, like 192.168.1.23', 'aria-label': 'Phone address',
    onkeydown: async (e) => {
      if (e.key === 'Escape') root.replaceChildren();
      if (e.key !== 'Enter') return;
      try { await api('connect', { address: input.value.trim() }); root.replaceChildren(); toast('Contacting your phone', 'progress', 4000); } catch (err) { toast(String(err), 'error'); }
    } });
  root.replaceChildren(h('div', { class: 'palette-backdrop', onclick: (e) => { if (e.target === e.currentTarget) root.replaceChildren(); } },
    h('div', { class: 'palette' }, h('label', { class: 'search' }, icon('wifi', 'sm'), input),
      h('p', { class: 'none', text: 'On the phone: KDE Connect, then About, shows its address under Wi-Fi settings. Useful on VPNs and guest networks where phones can\'t be found automatically.' }))));
  input.focus();
}

// ---------- actions ----------
async function quick(method, done, global = false) {
  try { await api(method, global ? {} : { device: state.deviceId }); toast(done); } catch (e) { toast(String(e), 'error'); }
}
const ringPhone = () => quick('ring', 'Ringing your phone');

// The background service syncs the clipboard itself when it can; copying here as well would move
// ownership to the window, and on X11 the text would vanish when the window closes.
async function phoneClipboard(text) {
  try { state.me = await api('identity'); } catch { /* use what we have */ }
  if (state.me && state.me.clipboardSync) toast('Copied from your phone');
  else copy(text, 'Copied from your phone');
}

async function copy(text, done) {
  try { await T.core.invoke('copy_text', { text }); toast(done); } catch (e) { toast(`Couldn't copy: ${e}`, 'error'); }
}

function toast(text, kind = '', ms = 2600) {
  const el = h('div', { class: `toast ${kind === 'error' ? 'error' : ''}` }, kind === 'progress' ? h('div', { class: 'spinner' }) : null, text);
  $('#toasts').append(el);
  setTimeout(() => el.remove(), kind === 'error' ? 5000 : ms);
  return el;
}

// ---------- command palette ----------
function commands() {
  const d = device();
  const live = d && d.connected;
  const player = activePlayer();
  const cmds = [
    { label: 'New message', sub: 'Ctrl+N', icon: 'plus', run: () => openNew() },
    live && { label: 'Ring my phone', icon: 'ring', run: ringPhone },
    live && { label: 'Ping my phone', icon: 'bolt', run: () => quick('ping', 'Ping sent') },
    live && player && { label: player.is_playing ? `Pause ${player.title}` : `Play ${player.title}`, sub: player.name, icon: player.is_playing ? 'pause' : 'play', run: () => mediaAction(player, 'PlayPause') },
    live && player && player.can_go_next && { label: 'Next song', sub: player.name, icon: 'next', run: () => mediaAction(player, 'Next') },
    state.call && { label: 'Mute the ringing call', icon: 'mute', run: () => quick('mute', 'Ringer muted') },
    { label: 'Look for my phone', icon: 'refresh', run: () => quick('refresh', 'Looking for your phone', true) },
    { label: 'Show notifications', icon: 'bell', run: toggleFeed },
    { label: 'Check for updates', icon: 'download', run: checkForUpdates },
    recentCode() && { label: `Copy code ${recentCode().code}`, sub: recentCode().from, icon: 'key', run: () => copy(recentCode().code, 'Code copied') },
  ].filter(Boolean);
  cmds.push({ label: 'Show phone apps', icon: 'apps', run: () => showView('apps') });
  if (apps.status?.device) {
    cmds.push({ label: 'Show phone screen', icon: 'phone', run: () => openApp(null) });
    for (const a of apps.list || []) cmds.push({ label: `Open ${a.name}`, sub: 'Phone app', icon: 'apps', run: () => openApp(a) });
  }
  for (const m of heads()) cmds.push({ label: threadTitle(m.addresses), sub: 'Conversation', icon: 'message', run: () => selectThread(m.thread_id) });
  for (const c of state.contacts) cmds.push({ label: `Text ${c.name}`, sub: prettyNumber(c.number), icon: 'send', run: () => openNew(c) });
  return cmds;
}

function openPalette() {
  const root = $('#palette-root');
  if (root.firstChild) return;
  let sel = 0, shown = [];
  const ul = h('ul', { role: 'listbox', 'aria-label': 'Commands' });
  const input = h('input', { placeholder: 'Type a command, a name or a conversation', 'aria-label': 'Command', oninput: () => { sel = 0; draw(); },
    onkeydown: (e) => {
      if (e.key === 'Escape') close();
      else if (e.key === 'ArrowDown') { e.preventDefault(); sel = Math.min(shown.length - 1, sel + 1); draw(); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); sel = Math.max(0, sel - 1); draw(); }
      else if (e.key === 'Enter' && shown[sel]) { close(); shown[sel].run(); }
    } });
  const close = () => { root.replaceChildren(); };
  const draw = () => {
    const q = input.value.trim().toLowerCase();
    shown = commands().filter((c) => !q || `${c.label} ${c.sub || ''}`.toLowerCase().includes(q)).slice(0, 9);
    ul.replaceChildren(...(shown.length ? shown.map((c, i) => h('li', { role: 'option', 'aria-selected': i === sel ? 'true' : 'false',
      onmousemove: () => { if (sel !== i) { sel = i; draw(); } }, onclick: () => { close(); c.run(); } },
    icon(c.icon), h('span', { text: c.label }), c.sub ? h('span', { class: 'sub', text: c.sub }) : null)) : [h('div', { class: 'none', text: 'No matches' })]));
  };
  root.replaceChildren(h('div', { class: 'palette-backdrop', onclick: (e) => { if (e.target === e.currentTarget) close(); } },
    h('div', { class: 'palette', role: 'dialog', 'aria-label': 'Commands' }, h('label', { class: 'search' }, icon('search', 'sm'), input), ul)));
  draw();
  input.focus();
}

document.addEventListener('keydown', (e) => {
  const mod = e.ctrlKey || e.metaKey;
  if (mod && (e.key === 'k' || e.key === 'p')) { e.preventDefault(); openPalette(); }
  else if (mod && e.key === 'n' && state.deviceId) { e.preventDefault(); openNew(); }
  else if (e.key === 'Escape' && state.feedOverlay) toggleFeed();
});

// ---------- drag and drop ----------
async function setupDrop() {
  const wv = T.webview?.getCurrentWebview?.();
  if (!wv) return;
  const overlay = $('#drop');
  await wv.onDragDropEvent(async (e) => {
    const d = device();
    const t = e.payload.type;
    if (t === 'enter' || t === 'over') {
      $('#drop-text').textContent = d && d.connected ? `Drop to send to ${d.name}` : 'Your phone is out of reach';
      overlay.hidden = false;
    } else if (t === 'leave') {
      overlay.hidden = true;
    } else if (t === 'drop') {
      overlay.hidden = true;
      if (!(d && d.connected)) return toast('Your phone is out of reach', 'error');
      for (const path of e.payload.paths || []) sendFile(path, d);
    }
  });
}

async function sendFile(path, d) {
  const name = path.split('/').pop();
  const t = toast(`Sending ${name}`, 'progress', 600000);
  try {
    await api('share_file', { device: d.id, path });
    t.remove();
    toast(`${name} is on ${d.name}`);
  } catch (e) {
    t.remove();
    toast(`${name} didn't send: ${e}`, 'error');
  }
}

// ---------- events ----------
function onEvent(ev) {
  if (ev.event === 'device') {
    const i = state.devices.findIndex((d) => d.id === ev.id);
    const before = i >= 0 ? state.devices[i] : null;
    if (i >= 0) state.devices[i] = ev; else state.devices.push(ev);
    if (!ev.connected && ev.pair === 'unpaired') state.devices = state.devices.filter((d) => d.id !== ev.id);
    const becamePaired = ev.pair === 'paired' && (!before || before.pair !== 'paired');
    const cameBack = ev.id === state.deviceId && ev.connected && before && !before.connected;
    const lost = ev.id === state.deviceId && ev.pair !== 'paired';
    if (lost) { state.deviceId = null; state.threads.clear(); state.notifications = {}; }
    if (becamePaired && !state.deviceId) { state.deviceId = ev.id; loadDevice().then(render); }
    else if (cameBack) loadDevice().then(render);
    if (!state.deviceId || !$('.rail')) return render();
    renderRail(); renderBanners();
    if (cameBack || !ev.connected) renderThreadPane(true);
    return;
  }
  if (ev.event !== 'update' || ev.device_id !== state.deviceId) return;
  const u = ev.update;
  switch (u.kind) {
    case 'battery': state.battery = u; renderRail(); break;
    case 'attachment': waitingAttachments.get(u.unique_identifier)?.resolve(u.path); waitingAttachments.delete(u.unique_identifier); break;
    case 'media': state.media = Object.fromEntries(u.players.map((p) => [p.name, p])); renderRail(); break;
    case 'signal': state.signal = u.signal; renderRail(); break;
    case 'notification': state.notifications[u.id] = u; renderFeed(); renderRail(); break;
    case 'notification_removed': delete state.notifications[u.id]; renderFeed(); renderRail(); break;
    case 'messages':
      mergeMessages(u.messages);
      if (u.messages.some((m) => m.thread_id === state.selected)) {
        state.loadingThread = null;
        const msgs = threadMessages(state.selected);
        state.seen[state.selected] = msgs[msgs.length - 1].date; saveSeen();
      }
      renderList(); renderRail(); renderThreadPane(false); renderFeed();
      break;
    case 'contacts':
      api('contacts', { device: state.deviceId }).then((c) => { state.contacts = c; renderList(); renderThreadPane(false); }).catch(() => {});
      break;
    case 'call': state.call = u.state === 'missed' ? null : u; renderRail(); break;
    case 'call_ended': state.call = null; renderRail(); break;
    case 'ping': toast(u.message ? `Phone: ${u.message}` : 'Ping from your phone'); break;
    case 'clipboard': phoneClipboard(u.content); break;
    case 'shared_text': copy(u.text, 'Text from your phone copied'); break;
    case 'shared_url': copy(u.url, 'Link from your phone copied'); break;
    case 'file_received': toast(`Saved ${u.path.split('/').pop()} to Downloads`); break;
    case 'transfer_failed': toast(`${u.name} didn't arrive: ${u.reason}`, 'error'); break;
  }
}

// Notice when the other KDE Connect app quits, so its warning goes away.
setInterval(async () => {
  if (!state.daemon || !state.me || state.me.discoveryListening !== false) return;
  try { state.me = await api('identity'); render(); } catch { /* daemon restarting */ }
}, 5000);

// Relative times ("5 min") go stale; refresh them once a minute.
setInterval(() => { if (state.deviceId && $('.rail')) { renderList(); renderFeed(); } }, 60000);

async function start() {
  await T.event.listen('tether', (e) => onEvent(e.payload));
  await T.event.listen('daemon', (e) => {
    const was = state.daemon;
    state.daemon = !!e.payload.connected;
    if (state.daemon && !was) { state.bootFailed = false; loadAll(); }
    else if (!state.daemon && was) render();
  });
  setTimeout(() => { if (!state.daemon) { state.bootFailed = true; render(); } }, 6000);
  // Belt and braces: if the "connected" event fired before we listened, a direct call still finds it.
  setInterval(async () => {
    if (state.daemon) return;
    try { await api('identity'); state.daemon = true; state.bootFailed = false; loadAll(); } catch { /* still starting */ }
  }, 1500);
  setupDrop();
  // The connection may already be up before we started listening.
  try { await api('identity'); state.daemon = true; loadAll(); } catch { render(); }
}

start();
