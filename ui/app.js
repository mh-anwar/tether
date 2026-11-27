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
  transfers: [],
  threads: new Map(), // thread_id -> Map(message id -> message)
  contacts: [], // [{name, number}] numbers are digits only
  selected: null, // thread id, or 'new'
  filter: '',
  pending: [], // outgoing texts not yet confirmed by the phone
  loadingThread: null,
  call: null,
  media: {}, // player name -> what it plays, from the phone
  feedOpen: loadPref('tether.feedOpen', false), // the Notifications column; closed unless the person opens it
  newTo: [],
  seen: loadSeen(),
  view: 'messages', // or 'apps'
};

// Per-window conveniences (panel open, column widths); losing them only resets the layout.
function loadPref(key, fallback) {
  try { const v = localStorage.getItem(key); return v === null ? fallback : JSON.parse(v); } catch { return fallback; }
}
function savePref(key, value) {
  try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* private mode */ }
}

// Texts the service will send later (scheduled_list), for every phone.
const scheduled = { items: [] };

function loadSeen() {
  try { return JSON.parse(localStorage.getItem('tether.seen') || '{}'); } catch { return {}; }
}
function saveSeen() {
  // The service draws the tray icon's unread count from this.
  if (state.daemon) api('seen_set', { seen: state.seen }).catch(() => {});
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
  // Nodes that are already in place stay untouched (lists reuse their rows), so the browser only
  // lays out what changed.
  const nodes = [build()].flat().filter(Boolean).map((n) => (n instanceof Node ? n : document.createTextNode(String(n))));
  nodes.forEach((n, i) => {
    const cur = container.childNodes[i];
    if (cur !== n) container.insertBefore(n, cur || null);
  });
  while (container.childNodes.length > nodes.length) container.lastChild.remove();
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

// Updates from the phone come in bursts (hundreds of texts on connect). Each marks what it touched;
// the screen is redrawn once per frame instead of once per packet.
const dirty = new Set();
let frame = 0;
function invalidate(...parts) {
  for (const p of parts) dirty.add(p);
  if (!frame) frame = requestAnimationFrame(flushRender);
}
function flushRender() {
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  const parts = new Set(dirty);
  dirty.clear();
  if (parts.has('list')) renderList();
  if (parts.has('rail')) renderRail();
  if (parts.has('thread')) renderThreadPane(false);
  if (parts.has('feed')) renderFeed();
}

// ---------- formatting ----------
const digits = (s) => String(s || '').replace(/\D/g, '');

// Names by full number and by the last 9 digits, rebuilt when the contact list is replaced. Looking
// through the whole list for every row made one incoming text cost a quarter second on a big phone.
const contactIndex = { src: null, exact: new Map(), tail: new Map() };
function nameFor(number) {
  const d = digits(number);
  if (!d) return number;
  if (contactIndex.src !== state.contacts) {
    contactIndex.src = state.contacts;
    contactIndex.exact.clear(); contactIndex.tail.clear();
    for (const c of state.contacts) {
      if (!contactIndex.exact.has(c.number)) contactIndex.exact.set(c.number, c.name);
      const t = c.number.slice(-9);
      if (!contactIndex.tail.has(t)) contactIndex.tail.set(t, c.name);
    }
  }
  const tail = d.slice(-9);
  const hit = contactIndex.exact.get(d) ?? (tail.length >= 7 ? contactIndex.tail.get(tail) : undefined);
  return hit ?? prettyNumber(number);
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
  loadCalls();
  api('computer_status').then((c) => { state.computerRinging = c.ringing; if (c.commands) openCommandsSettings(); renderRail(); }).catch(() => {});
  loadUpdate();
  api('seen_set', { seen: state.seen }).catch(() => {});
  if (state.view === 'settings') loadSettings();
  if (state.deviceId) await loadDevice();
  loadScheduled();
  render();
  // A window started from the tray menu ("Send files...") picks up what it was opened for.
  api('ui_pending').then((ev) => { if (ev) onEvent(ev); }).catch(() => {});
}

async function loadDevice() {
  const id = state.deviceId;
  try {
    const [data, threads, contacts] = await Promise.all([
      api('device_data', { device: id }), api('threads', { device: id }), api('contacts', { device: id }),
    ]);
    if (state.deviceId !== id) return; // another phone was picked meanwhile
    const received = await api('received_list');
    state.transfers = [...state.transfers.filter((t) => !t.path), ...received];
    state.battery = data.battery;
    state.signal = data.signal || {};
    state.notifications = data.notifications || {};
    state.call = data.call;
    state.media = data.media || {};
    state.keyboardActive = data.keyboard_active;
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
      state.bootFailed && h('p', { class: 'hint', text: 'Tether keeps trying to start it. If this stays, close Tether and open it again.' })));
    return;
  }
  if (!state.deviceId) return renderPairPage(app);
  if (!$('.rail', app)) buildLayout(app);
  app.className = `app ${['files', 'contacts', 'keyboard', 'devices'].includes(state.view) ? 'view-tools' : ''} ${state.feedOpen ? 'feed-open' : 'no-feed'} ${state.view === 'apps' ? 'view-apps' : ''} ${state.view === 'photos' ? 'view-photos' : ''} ${state.view === 'settings' ? 'view-settings' : ''}`;
  renderRail();
  if (state.view === 'apps') renderApps();
  else if (state.view === 'photos') renderPhotos();
  else if (state.view === 'settings') renderSettings();
  else if (['files', 'contacts', 'keyboard', 'devices'].includes(state.view)) renderTools();
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
            oninput: (e) => { state.filter = e.target.value; listLimit = LIST_STEP; renderList(); },
            onkeydown: (e) => { if (e.key === 'ArrowDown') { e.preventDefault(); $('.thread-item')?.focus(); } } }))),
      h('div', { class: 'threads', role: 'listbox', 'aria-label': 'Conversations', onkeydown: listKeys, onscroll: listScroll })),
    h('main', { class: 'thread' }),
    h('main', { class: 'apps-page', 'aria-label': 'Phone apps' }),
    h('main', { class: 'photos-page', 'aria-label': 'Phone photos' }),
    h('main', { class: 'tools-page', 'aria-label': 'Phone tools' }),
    h('main', { class: 'settings-page', 'aria-label': 'Settings' }),
    h('aside', { class: 'feed', 'aria-label': 'Notifications' }),
    resizer('rail', 'Resize the sidebar'),
    resizer('list', 'Resize the conversation list'),
    resizer('feed', 'Resize the notifications panel'),
  );
  applyWidths();
}

// ---------- column widths ----------
// Dragging a handle sets a CSS variable; the grid and the handles' positions both read it.
const WIDTHS = { rail: [232, 180, 360], list: [300, 220, 520], feed: [320, 260, 520] }; // default, min, max
const widths = loadPref('tether.widths', {});
const clampWidth = (part, px) => Math.round(Math.min(WIDTHS[part][2], Math.max(WIDTHS[part][1], px)));
function applyWidths() {
  const app = $('#app');
  for (const part of Object.keys(WIDTHS)) {
    const px = clampWidth(part, Number(widths[part]) || WIDTHS[part][0]);
    app.style.setProperty(`--${part}-w`, `${px}px`);
    const handle = $(`.resizer[data-part="${part}"]`, app);
    if (handle) handle.setAttribute('aria-valuenow', String(px));
  }
}
function setWidth(part, px) {
  if (px === null) delete widths[part]; else widths[part] = clampWidth(part, px);
  savePref('tether.widths', widths);
  applyWidths();
}
function resetWidths() {
  for (const part of Object.keys(WIDTHS)) delete widths[part];
  savePref('tether.widths', widths);
  applyWidths();
}
const currentWidth = (part) => clampWidth(part, Number(widths[part]) || WIDTHS[part][0]);
function resizer(part, label) {
  const [, min, max] = WIDTHS[part];
  // The feed sits on the right, so dragging left makes it wider.
  const dir = part === 'feed' ? -1 : 1;
  return h('div', { class: 'resizer', 'data-part': part, role: 'separator', tabindex: '0', 'aria-orientation': 'vertical',
    'aria-label': label, title: `${label}. Double-click to reset.`, 'aria-valuemin': String(min), 'aria-valuemax': String(max),
    ondblclick: () => setWidth(part, null),
    onkeydown: (e) => {
      const step = e.shiftKey ? 48 : 16;
      if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') { e.preventDefault(); setWidth(part, currentWidth(part) + (e.key === 'ArrowRight' ? step : -step) * dir); }
      else if (e.key === 'Home') { e.preventDefault(); setWidth(part, null); }
    },
    onpointerdown: (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      const handle = e.currentTarget, startX = e.clientX, start = currentWidth(part);
      handle.setPointerCapture(e.pointerId);
      document.body.classList.add('resizing');
      const move = (ev) => {
        widths[part] = clampWidth(part, start + (ev.clientX - startX) * dir);
        applyWidths();
      };
      const up = () => {
        handle.removeEventListener('pointermove', move);
        document.body.classList.remove('resizing');
        savePref('tether.widths', widths);
      };
      handle.addEventListener('pointermove', move);
      handle.addEventListener('pointerup', up, { once: true });
      handle.addEventListener('pointercancel', up, { once: true });
    } });
}

const lowBattery = () => !!state.battery && !state.battery.charging && (state.battery.low || state.battery.charge <= 15);
// What the phone reports about itself, for the Devices page (only the selected phone's data is loaded).
function phoneFacts(d) {
  if (d.id !== state.deviceId || !d.connected) return '';
  const signal = Object.values(state.signal)[0], b = state.battery;
  return [b && `Battery ${b.charge}%${b.charging ? ', charging' : ''}${lowBattery() ? ', low' : ''}`, signal && `${signal.network_type} signal`].filter(Boolean).join(' · ');
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
        state.battery && h('span', { title: 'Battery', class: lowBattery() ? 'low' : '' }, batteryIcon(state.battery.charge, state.battery.charging), `${state.battery.charge}%${state.battery.charging ? ' · Charging' : ''}${lowBattery() ? ' · Low battery' : ''}`),
        ...Object.values(state.signal).slice(0, 1).map((s) => h('span', { title: 'Signal' }, signalIcon(s.strength), s.network_type))) : null,
      h('div', { class: 'conn' },
        h('span', { class: `dot ${d && d.connected ? 'on' : 'wait'}` }),
        d && d.connected ? `Connected${d.address ? ' over network' : ''}` : (d?.last_seen ? `Last seen ${ago(d.last_seen)} · Reconnecting` : 'Out of reach. Reconnecting')),
      h('div', { class: 'card-actions' },
        h('button', { class: 'btn', disabled: !(d && d.connected), onclick: ringPhone, title: 'Ring the phone at full volume' }, icon('ring', 'sm'), 'Ring'),
        h('button', { class: 'btn', disabled: !(d && d.connected), onclick: () => quick('ping', 'Ping sent'), title: 'Send a ping' }, icon('bolt', 'sm'), 'Ping'))),
    state.call && state.call.state === 'ringing' && !btCallsLive() ? h('div', { class: 'call' },
      icon('call', 'pulse'),
      h('div', {}, h('b', { text: state.call.name || prettyNumber(state.call.number || '') || 'Unknown caller' }), h('span', { text: 'Incoming call' })),
      h('button', { class: 'btn', onclick: () => quick('mute', 'Ringer muted') }, icon('mute', 'sm'), 'Mute')) : null,
    state.devices.filter((x) => x.pair === 'paired').length > 1 ? h('select', { 'aria-label': 'Phone', onchange: (e) => switchDevice(e.target.value) },
      state.devices.filter((x) => x.pair === 'paired').map((x) => h('option', { value: x.id, selected: x.id === state.deviceId, text: x.name }))) : null,
    ...state.devices.filter((x) => x.pair === 'requested_by_peer').map((x) => h('div', { class: 'contact-card' },
      h('b', { text: `${x.name} wants to pair` }), h('span', { text: `Check this code on your phone: ${x.verification_key || ''}` }),
      toolButton('Accept', () => pairAction('accept', x.id)), toolButton('Decline', () => pairAction('reject', x.id)))),
    state.computerRinging ? h('div', { class: 'call finding', role: 'status' },
      icon('ring', 'pulse'),
      h('b', { text: 'Your phone is finding this computer' }),
      h('button', { class: 'btn primary small', onclick: () => api('computer_ring_stop').catch((e) => toast(String(e), 'error')) }, 'Stop')) : null,
    mediaCard(),
    h('nav', { class: 'nav' },
      h('button', { 'aria-current': state.view === 'messages' ? 'page' : null, onclick: () => showView('messages') }, icon('message'), h('span', { class: 'label', text: 'Messages' }),
        unread ? h('span', { class: 'count label', text: String(unread), 'aria-label': `${unread} unread` }) : null),
      h('button', { 'aria-current': state.view === 'apps' ? 'page' : null, onclick: () => showView('apps') }, icon('apps'), h('span', { class: 'label', text: 'Apps' })),
      h('button', { 'aria-current': state.view === 'photos' ? 'page' : null, onclick: () => showView('photos') }, icon('image'), h('span', { class: 'label', text: 'Photos' })),
      ...[['files', 'Files', 'folder'], ['contacts', 'Contacts', 'user'], ['keyboard', 'Keyboard', 'keyboard'], ['devices', 'Devices', 'phone']].map(([view, label, glyph]) =>
        h('button', { title: label, 'aria-label': label, 'aria-current': state.view === view ? 'page' : null, onclick: () => showView(view) }, icon(glyph), h('span', { class: 'label', text: label }))),
      h('button', { 'data-key': 'nav-notifications', 'aria-pressed': String(state.feedOpen), title: state.feedOpen ? 'Hide notifications' : 'Show notifications', onclick: toggleFeed }, icon('bell'), h('span', { class: 'label', text: 'Notifications' }),
        notes ? h('span', { class: 'count label', text: String(notes) }) : null),
      h('button', { 'aria-current': state.view === 'settings' ? 'page' : null, onclick: () => showView('settings') }, icon('settings'), h('span', { class: 'label', text: 'Settings' }))),
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
      // Texts take a few seconds to arrive after connecting, so the setup steps sit under the spinner.
      const missing = d?.readiness?.some((f) => f.id === 'texts' && !f.ready);
      return h('div', { class: 'empty' },
        d && d.connected && !missing ? h('div', { class: 'spinner' }) : icon('message'),
        h('p', { text: d && d.connected ? (missing ? 'Texts are not available on this phone.' : 'Loading conversations') : 'Conversations appear when your phone is in reach.' }), readinessHint('texts'));
    }
    if (!items.length) return h('div', { class: 'empty' }, h('p', { text: `Nothing matches "${state.filter}".` }));
    // The selected conversation is always drawn, however far down it is.
    const shown = items.slice(0, Math.max(listLimit, items.findIndex((m) => m.thread_id === state.selected) + 1));
    listMore = shown.length < items.length;
    const rows = new Map();
    const nodes = shown.map((m) => {
      const title = threadTitle(m.addresses);
      const unread = isUnread(m);
      const preview = (m.type === 2 ? 'You: ' : '') + (m.body || (m.attachments?.length ? 'Picture' : ''));
      const selected = state.selected === m.thread_id;
      const time = shortTime(m.date);
      // Rows that look the same as last time are reused, so one new text builds one row, not all.
      const sig = `${m._id}|${title}|${preview}|${time}|${unread}|${selected}`;
      const old = listRows.get(m.thread_id);
      const row = old && old.sig === sig ? old : { sig, el: threadRow(m, title, preview, time, unread, selected) };
      rows.set(m.thread_id, row);
      return row.el;
    });
    listRows = rows;
    return nodes;
  });
}

let listRows = new Map(); // thread id -> { sig, el }
// Only the rows near what is on screen are drawn; scrolling towards the end draws the next batch.
// Hundreds of rows made every new text re-lay out the whole list.
const LIST_STEP = 80;
let listLimit = LIST_STEP;
let listMore = false;
function listScroll(e) {
  const box = e.currentTarget;
  if (listMore && box.scrollHeight - box.scrollTop - box.clientHeight < 600) { listLimit += LIST_STEP; renderList(); }
}

function threadRow(m, title, preview, time, unread, selected) {
  return h('button', {
    class: `thread-item ${unread ? 'unread' : ''}`, role: 'option', 'data-key': `t${m.thread_id}`,
    'aria-selected': selected ? 'true' : 'false', onclick: () => selectThread(m.thread_id),
  },
  avatar(title, m.addresses.length > 1),
  h('div', { style: 'min-width:0' }, h('div', { class: 'who', text: title }), h('div', { class: 'preview', text: preview })),
  h('div', { class: 'meta' }, h('time', { text: time }), unread ? h('span', { class: 'unread-dot', 'aria-label': 'Unread' }) : null));
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
      addrs.length === 1 ? callButton(addrs[0].address, title) : null,
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
  nodes.push(...scheduledNodes(id, msgs));
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
      laterButton(ta, d),
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
  if (ta) { ta.value = m.body; send(ta); }
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

// A picture in a dialog: `buttons` go in its bar, `load` resolves to the element to show.
function openViewer(label, buttons, load, retry, waiting = 'Getting it from your phone') {
  const root = $('#palette-root');
  const back = document.activeElement;
  const close = () => { root.replaceChildren(); back?.focus?.(); };
  const body = h('div', { class: 'viewer-body', 'aria-live': 'polite' }, h('div', { class: 'spinner' }), h('p', { text: waiting }));
  const closeBtn = h('button', { class: 'btn ghost icon-only', title: 'Close (Esc)', 'aria-label': 'Close', onclick: close }, icon('x'));
  for (const b of buttons) b.disabled = true;
  root.replaceChildren(h('div', { class: 'palette-backdrop viewer-backdrop', onclick: (e) => { if (e.target === e.currentTarget) close(); },
    onkeydown: (e) => { if (e.key === 'Escape') close(); } },
  h('div', { class: 'viewer', role: 'dialog', 'aria-modal': 'true', 'aria-label': label },
    h('div', { class: 'viewer-bar' }, buttons, closeBtn), body)));
  closeBtn.focus();
  load().then((el) => {
    body.replaceChildren(el);
    for (const b of buttons) b.disabled = false;
  }).catch((e) => {
    body.replaceChildren(icon('alert', 'lg'), h('p', { text: e.message || String(e) }),
      h('button', { class: 'btn', onclick: () => { close(); retry(); } }, icon('refresh', 'sm'), 'Try again'));
  });
}

// ---------- camera ----------
// Older KDE Connect Android releases can open the camera for the computer; newer ones dropped it,
// so the command only shows for phones that say they can.
const canTakePhoto = (d) => !!(d && d.connected && (d.incoming_capabilities || []).includes('kdeconnect.photo.request'));
let photoWait = null; // the open "Take a photo" viewer, waiting for the phone's picture

function takePhoto() {
  const d = device();
  if (!canTakePhoto(d)) { toast('Your phone can\'t take photos for this computer', 'error'); return; }
  let saved = null;
  const act = (method) => async () => { try { await api(method, { path: saved }); } catch (e) { toast(String(e), 'error'); } };
  const openBtn = h('button', { class: 'btn', 'data-key': 'camera-open', onclick: act('received_open') }, icon('image', 'sm'), 'Open');
  const folder = h('button', { class: 'btn', 'data-key': 'camera-folder', onclick: act('received_show') }, icon('folder', 'sm'), 'Show in folder');
  openViewer('Photo from your phone', [openBtn, folder], async () => {
    const path = await new Promise((resolve, reject) => {
      const t = setTimeout(() => { photoWait = null; reject(new Error('No photo came from your phone. Try again with the phone unlocked.')); }, 180000);
      photoWait = { resolve: (p) => { clearTimeout(t); photoWait = null; resolve(p); }, reject: (e) => { clearTimeout(t); photoWait = null; reject(e); } };
      api('photo_take', { device: d.id }).catch((e) => photoWait?.reject(e));
    });
    saved = path;
    const preview = await api('photo_preview', { path });
    return h('img', { src: T.core.convertFileSrc(preview), alt: 'Photo from your phone' });
  }, takePhoto, `Take the photo on ${d.name}`);
}

function openAttachment(a) {
  const save = h('button', { class: 'btn', onclick: async () => {
    try { const r = await api('sms_attachment_save', { device: state.deviceId, unique_identifier: a.unique_identifier }); toast(`Saved to ${r.path.split('/').slice(-2).join('/')}`); } catch (e) { toast(String(e), 'error'); }
  } }, icon('download', 'sm'), 'Save to Downloads');
  openViewer('Picture message', [save], async () => {
    const src = T.core.convertFileSrc(await fetchAttachment(a));
    const kind = a.mime_type || '';
    return kind.startsWith('video/') ? h('video', { src, controls: true, autoplay: true })
      : kind.startsWith('image/') ? h('img', { src, alt: 'Picture from the conversation' })
        : h('p', { text: 'Downloaded. Save it to open it with another app.' });
  }, () => openAttachment(a));
}

// ---------- settings ----------
const settings = { data: null, error: '', forgetting: false, saving: false, openCommands: false, tab: loadPref('tether.settingsTab', 'general') };

function openCommandsSettings() { settings.openCommands = true; settings.tab = 'access'; showView('settings'); }

async function loadSettings() {
  try { settings.data = await api('settings_get'); settings.error = ''; } catch (e) { settings.error = e.message || String(e); }
  renderSettings();
  if (settings.openCommands && document.getElementById('run-commands')) {
    settings.openCommands = false;
    document.getElementById('run-commands').scrollIntoView();
  }
}

// Saves one or more settings. Some need the background service to restart; it comes back by itself.
async function saveSettings(changes, done = 'Saved') {
  settings.saving = true;
  try {
    const r = await api('settings_set', changes);
    toast(r.restart ? `${done}. Reconnecting to your phone` : done);
    if (!r.restart) await loadSettings();
    return true;
  } catch (e) {
    toast(e.message || String(e), 'error');
    await loadSettings();
    return false;
  } finally { settings.saving = false; }
}

function settingSwitch(key, label, hint) {
  const input = h('input', { type: 'checkbox', role: 'switch', class: 'switch', 'data-key': `set-${key}`, checked: settings.data[key],
    onchange: (e) => saveSettings({ [key]: e.target.checked }) });
  return h('label', { class: 'setting check' }, h('span', {}, h('b', { text: label }), hint ? h('small', { text: hint }) : null), input);
}

// `list`: the setting is a list of names, shown and edited as comma-separated text.
function settingText(key, label, hint, button = 'Save', list = false) {
  const current = list ? (settings.data[key] || []).join(', ') : String(settings.data[key]);
  const input = h('input', { type: 'text', 'data-key': `set-${key}`, value: current, 'aria-label': label, placeholder: list ? 'App names, separated by commas' : null });
  return h('form', { class: 'setting', onsubmit: (e) => {
    e.preventDefault();
    const value = list ? input.value.split(',').map((v) => v.trim()).filter(Boolean) : input.value.trim();
    if (list || value !== current) saveSettings({ [key]: value });
  } },
    h('label', {}, h('b', { text: label }), hint ? h('small', { text: hint }) : null),
    h('div', { class: 'field-row' }, input, h('button', { class: 'btn', type: 'submit' }, button)));
}

function commandsEditor(commands, flatpak) {
  const row = (command, index) => {
    const name = h('input', { type: 'text', 'aria-label': 'Command name', value: command.name, required: true });
    const shell = h('input', { type: 'text', 'aria-label': 'Shell command', value: command.command, required: true });
    return h('form', { class: 'setting command-editor', 'data-key': `command-${command.id || 'new'}`, onsubmit: (e) => {
      e.preventDefault();
      const entry = { id: command.id || crypto.randomUUID(), name: name.value.trim(), command: shell.value.trim() };
      if (!entry.name || !entry.command) return;
      const next = [...commands]; if (index < 0) next.push(entry); else next[index] = entry;
      saveSettings({ commands: next });
    } }, h('label', {}, h('b', { text: index < 0 ? 'New command' : command.name }), h('small', { text: 'Name' }), name),
      h('label', {}, h('small', { text: 'Shell command' }), shell),
      h('div', { class: 'field-row' }, h('button', { class: 'btn', type: 'submit' }, index < 0 ? 'Add command' : 'Save command'),
        index >= 0 ? h('button', { class: 'btn', type: 'button', onclick: () => saveSettings({ commands: commands.filter((_, i) => i !== index) }) }, 'Remove command') : null));
  };
  return h('section', { id: 'run-commands' }, h('h2', { text: 'Commands your phone can run' }),
    h('p', { class: 'sub', text: flatpak
      ? "Commands run inside Tether's Flatpak sandbox, so most of your system's programs aren't there. Only add commands you want your paired phone to run."
      : 'Commands run on this computer with your account. Only add commands you want your paired phone to run.' }),
    commands.length ? commands.map(row) : h('p', { class: 'none', text: 'No commands yet.' }), row({ name: '', command: '' }, -1));
}

const SETTINGS_TABS = [['general', 'General'], ['notifications', 'Notifications'], ['calls', 'Calls'],
  ['access', 'Phone access'], ['files', 'Sharing'], ['connection', 'Connection']];
function settingsTabs() {
  const pick = (id, focus) => {
    settings.tab = id; savePref('tether.settingsTab', id); renderSettings();
    if (focus) $(`.settings-tabs [data-tab="${id}"]`)?.focus();
  };
  return h('div', { class: 'settings-tabs', role: 'tablist', 'aria-label': 'Settings sections',
    onkeydown: (e) => {
      const i = SETTINGS_TABS.findIndex(([id]) => id === settings.tab);
      const next = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: SETTINGS_TABS.length - 1 }[e.key];
      if (next === undefined) return;
      e.preventDefault();
      pick(SETTINGS_TABS[(next + SETTINGS_TABS.length) % SETTINGS_TABS.length][0], true);
    } },
  SETTINGS_TABS.map(([id, label]) => h('button', { role: 'tab', id: `tab-${id}`, 'data-tab': id, 'aria-controls': 'settings-panel',
    'aria-selected': String(settings.tab === id), tabindex: settings.tab === id ? '0' : '-1', onclick: () => pick(id) }, label)));
}
// A titled group of settings drawn as one card.
const group = (title, sub, ...rows) => h('section', { class: 'settings-group' },
  title ? h('h2', { text: title }) : null, sub ? h('p', { class: 'sub', text: sub }) : null,
  h('div', { class: 'settings-card' }, ...rows));

async function quitTether() {
  try { await api('quit'); } catch (e) { toast(e.message || String(e), 'error'); }
}

function settingsPanel(s, d, flatpak) {
  const addr = h('input', { type: 'text', 'data-key': 'set-address', placeholder: '192.168.1.20', 'aria-label': "Phone's address" });
  switch (settings.tab) {
    case 'notifications': return [
      group('From your phone', '',
        settingSwitch('mirror_notifications', 'Show phone notifications on this computer', 'They always show in the Tether window.'),
        ...quietSettings()),
      // The sandbox's filtered session bus can't watch other apps' notifications.
      flatpak ? null : group('To your phone', '',
        settingSwitch('send_notifications', "Send this computer's notifications to the phone", 'Off until you turn it on. Tether notifications are excluded.'),
        settingText('notification_blocklist', 'Apps that stay on this computer', 'Notifications from these apps are not sent to the phone.', 'Save', true))];
    case 'calls': return [
      group('During phone calls', 'What happens to music and videos playing on this computer.',
        h('div', { class: 'setting row-setting' }, h('label', { for: 'pause-media' }, h('b', { text: 'Desktop players' }), h('small', { text: 'Players you were listening to.' })),
          h('select', { id: 'pause-media', 'aria-label': 'Pause media during calls', onchange: (e) => saveSettings({ pause_media: e.target.value }) },
            [['ringing', 'Pause when the phone rings'], ['talking', 'Pause when the call is answered'], ['off', 'Keep playing']].map(([value, label]) => h('option', { value, selected: s.pause_media === value }, label)))),
        settingSwitch('mute_media', 'Mute instead of pausing', 'Restores each playing player’s volume when the call ends.'))];
    case 'access': return [
      group('What your phone can do on this computer', 'Each one also needs its switch on in KDE Connect on the phone.',
        settingSwitch('allow_commands', 'Run commands'),
        settingSwitch('allow_input', 'Control the mouse and keyboard'),
        settingSwitch('allow_lock', 'Lock the screen'),
        settingSwitch('allow_volume', 'Change the volume'),
        settingSwitch('allow_ring', 'Ring this computer')),
      commandsEditor(s.commands || [], flatpak)];
    case 'files': return [
      group('Clipboard', '', settingSwitch('clipboard_sync', 'Share the clipboard', 'What you copy on one is ready to paste on the other.')),
      group('Files', '', settingText('downloads_dir', 'Save files from the phone in', flatpak ? 'The whole path of a folder inside Downloads.' : 'The whole path of a folder.'))];
    case 'connection': return [
      group('Finding your phone', "Tether finds your phone by itself on the same Wi-Fi. On a network where it can't (a VPN, some office or guest Wi-Fi), add the phone's address. It is in the phone's Wi-Fi settings, under the network's details.",
        settingSwitch('mdns', 'Find phones with mDNS', 'Helps find phones on networks that block broadcast discovery.'),
        s.addresses.length ? h('ul', { class: 'addr-list' }, s.addresses.map((a) => h('li', {}, h('span', { text: a }),
          h('button', { class: 'btn ghost icon-only', 'aria-label': `Remove ${a}`, title: 'Remove', onclick: () => saveSettings({ addresses: s.addresses.filter((x) => x !== a) }, 'Removed') }, icon('trash', 'sm'))))) : null,
        h('form', { class: 'setting', onsubmit: (e) => { e.preventDefault(); const v = addr.value.trim(); if (v && !s.addresses.includes(v)) saveSettings({ addresses: [...s.addresses, v] }, 'Added'); } },
          h('label', { for: 'set-address' }, h('b', { text: "Phone's address" })),
          h('div', { class: 'field-row' }, addr, h('button', { class: 'btn', type: 'submit' }, icon('plus', 'sm'), 'Add address')))),
      d ? group(d.name, '',
        settings.forgetting
          ? h('div', { class: 'setting' }, h('p', { class: 'sub', text: `Forget ${d.name}? Its texts and notifications leave this computer, and you pair again to bring it back.` }),
            h('div', { class: 'field-row' },
              h('button', { class: 'btn danger', 'data-key': 'set-forget-yes', onclick: forgetPhone }, 'Forget this phone'),
              h('button', { class: 'btn', onclick: () => { settings.forgetting = false; renderSettings(); } }, 'Keep it')))
          : h('div', { class: 'setting row-setting' }, h('span', {}, h('b', { text: 'Forget this phone' }), h('small', { text: 'Unpairs it. Pair again to bring it back.' })),
            h('button', { class: 'btn', 'data-key': 'set-forget', onclick: () => { settings.forgetting = true; renderSettings(); } }, icon('trash', 'sm'), 'Forget'))) : null];
    default: return [
      s.tray_missing && !s.tray_note_dismissed ? trayNote() : null,
      group('This computer', '',
        settingText('device_name', 'Name', 'What your phone calls this computer.'),
        settingSwitch('autostart', 'Start Tether when I log in', 'Texts, notifications and the clipboard keep working with the window closed.')),
      group('Updates', '',
        flatpak ? null : settingSwitch('auto_update', 'Install new versions by themselves', 'Off: Tether still tells you when there is one.'),
        state.update?.kind === 'store' ? h('p', { class: 'sub', text: 'Your software store keeps Tether up to date.' })
          : h('div', { class: 'setting row-setting' }, h('span', {}, h('b', { text: `Tether ${s.version}` })),
            h('button', { class: 'btn', onclick: checkForUpdates }, icon('refresh', 'sm'), 'Check for updates'))),
      group('Window', '',
        h('div', { class: 'setting row-setting' }, h('span', {}, h('b', { text: 'Column widths' }), h('small', { text: 'Drag the edge of a column to resize it.' })),
          h('button', { class: 'btn', 'data-key': 'reset-widths', onclick: () => { resetWidths(); toast('Column widths reset'); } }, 'Reset')),
        h('div', { class: 'setting row-setting' }, h('span', {}, h('b', { text: 'Quit Tether' }), h('small', { text: 'Stops texts, notifications and the clipboard until you open Tether again. The tray icon’s menu has this too.' })),
          h('button', { class: 'btn', 'data-key': 'quit-tether', onclick: quitTether }, 'Quit')))];
  }
}

function renderSettings() {
  const page = $('.settings-page');
  if (!page || state.view !== 'settings') return;
  const d = device();
  const s = settings.data;
  if (!SETTINGS_TABS.some(([id]) => id === settings.tab)) settings.tab = 'general';
  swap(page, () => {
    const head = h('div', { class: 'apps-head settings-head' }, h('div', {}, h('h1', { text: 'Settings' })), s && !settings.error ? settingsTabs() : null);
    if (settings.error) return [head, h('div', { class: 'empty' }, icon('alert'), h('p', { text: settings.error }),
      h('button', { class: 'btn', onclick: loadSettings }, icon('refresh', 'sm'), 'Try again'))];
    if (!s) return [head, h('div', { class: 'empty' }, h('div', { class: 'spinner' }), h('p', { text: 'Loading settings' }))];
    return [head, h('div', { class: 'settings-body', id: 'settings-panel', role: 'tabpanel', 'aria-labelledby': `tab-${settings.tab}` },
      ...settingsPanel(s, d, !!state.me?.flatpak))];
  });
}

// GNOME has no tray unless an extension adds one; say so once, until it is closed.
function trayNote() {
  return h('section', { 'data-key': 'tray-note', 'aria-label': 'No tray on this desktop' }, h('h2', { text: 'Tray icon' }),
    h('div', { class: 'setting' },
      h('b', { text: 'Your desktop has no tray, so the Tether icon has nowhere to show' }),
      h('small', { text: 'GNOME shows tray icons with the AppIndicator extension, at extensions.gnome.org/extension/615. Without it, Tether keeps running when you close this window, and the unread count shows on its dock icon if your dock supports badges.' }),
      h('div', { class: 'field-row' },
        h('button', { class: 'btn', 'data-key': 'tray-extension', onclick: () => api('tray_extension_open').catch((e) => toast(e.message || String(e), 'error')) }, 'Get the extension'),
        h('button', { class: 'btn', 'data-key': 'tray-note-close', onclick: () => saveSettings({ tray_note_dismissed: true }, 'Note closed') }, 'Close this note'))));
}

async function forgetPhone() {
  settings.forgetting = false;
  try { await api('unpair', { device: state.deviceId }); toast('Phone forgotten'); } catch (e) { toast(e.message || String(e), 'error'); renderSettings(); }
}

// ---------- photos ----------
const photos = {
  status: 'idle', // idle | loading | ready | error
  error: '',
  folders: [], // [{ name, items: [path on the phone] }]
  tab: 0,
  thumbs: new Map(), // path on the phone -> local preview file, or null when it couldn't be made
  tiles: new Map(), // path on the phone -> its tile, updated in place as previews arrive
  pumping: false,
};

async function loadPhotos() {
  const id = state.deviceId;
  photos.status = 'loading';
  renderPhotos();
  try {
    const r = await api('photos_list', { device: id });
    if (state.deviceId !== id) return;
    photos.folders = r.folders;
    photos.status = 'ready';
    if (!photos.folders[photos.tab]?.items.length) photos.tab = Math.max(0, photos.folders.findIndex((f) => f.items.length));
  } catch (e) {
    photos.status = 'error';
    photos.error = e.message || String(e);
  }
  renderPhotos();
  pumpThumbs();
}

// Previews are made a few at a time, newest first, for the folder being looked at.
async function pumpThumbs() {
  if (photos.pumping) return;
  photos.pumping = true;
  try {
    for (;;) {
      if (state.view !== 'photos' || photos.status !== 'ready') return;
      const id = state.deviceId;
      const want = (photos.folders[photos.tab]?.items || []).filter((p) => !photos.thumbs.has(p)).slice(0, 6);
      if (!want.length) return;
      let got = {};
      try { got = await api('photos_thumbs', { device: id, paths: want }); } catch (e) {
        if (state.deviceId === id) { photos.status = 'error'; photos.error = e.message || String(e); renderPhotos(); }
        return;
      }
      if (state.deviceId !== id) return;
      for (const p of want) {
        photos.thumbs.set(p, got[p] || null);
        const tile = photos.tiles.get(p);
        if (tile) fillTile(tile, p);
      }
    }
  } finally { photos.pumping = false; }
}

const photoName = (path) => path.split('/').pop();

function fillTile(tile, path) {
  const thumb = photos.thumbs.get(path);
  tile.classList.toggle('waiting', thumb === undefined);
  if (thumb) tile.replaceChildren(h('img', { src: T.core.convertFileSrc(thumb), alt: '', loading: 'lazy' }));
  else if (thumb === null) tile.replaceChildren(icon('image'), h('span', { text: 'No preview' }));
  else tile.replaceChildren();
}

function photoTile(path) {
  let tile = photos.tiles.get(path);
  if (!tile) {
    tile = h('button', { class: 'photo-tile', 'data-key': `p${path}`, 'aria-label': photoName(path), title: photoName(path), onclick: () => openPhoto(path) });
    photos.tiles.set(path, tile);
    fillTile(tile, path);
  }
  return tile;
}

function renderPhotos() {
  const page = $('.photos-page');
  if (!page || state.view !== 'photos') return;
  const d = device();
  swap(page, () => {
    const head = h('div', { class: 'apps-head' },
      h('div', { class: 'row' },
        h('div', {}, h('h1', { text: 'Photos' }), h('p', { class: 'sub', text: `The newest pictures on ${d?.name || 'your phone'}. Open one to copy or save it.` })),
        h('div', { class: 'row' },
          canTakePhoto(d) ? h('button', { class: 'btn', 'data-key': 'photos-camera', onclick: takePhoto }, icon('camera', 'sm'), 'Take a photo') : null,
          h('button', { class: 'btn', 'data-key': 'photos-refresh', disabled: photos.status === 'loading', onclick: loadPhotos }, icon('refresh', 'sm'), 'Refresh'))),
      photos.status === 'ready' ? h('div', { class: 'tabs', role: 'tablist', 'aria-label': 'Kind of picture' },
        photos.folders.map((f, i) => h('button', { role: 'tab', 'data-key': `photos-tab-${i}`, 'aria-selected': photos.tab === i ? 'true' : 'false',
          onclick: () => { photos.tab = i; renderPhotos(); pumpThumbs(); } }, f.name, h('span', { class: 'count', text: String(f.items.length) })))) : null);
    if (device()?.readiness?.some((f) => f.id === 'sftp' && !f.ready)) return [head, h('div', { class: 'empty' }, icon('folder'), readinessHint('sftp'))];
    if (photos.status === 'loading' || photos.status === 'idle') return [head, h('div', { class: 'empty' }, h('div', { class: 'spinner' }), h('p', { text: 'Getting your pictures from the phone' }))];
    if (photos.status === 'error') return [head, h('div', { class: 'empty' }, icon('alert'), h('p', { text: photos.error }), h('p', { text: phoneSetup('sftp') }),
      h('button', { class: 'btn', 'data-key': 'photos-retry', onclick: loadPhotos }, icon('refresh', 'sm'), 'Try again'))];
    const folder = photos.folders[photos.tab];
    if (!folder || !folder.items.length) return [head, h('div', { class: 'empty' }, icon('image'), h('p', { text: 'No pictures here yet.' }))];
    return [head, h('div', { class: 'photo-grid', role: 'tabpanel', 'aria-label': folder.name }, folder.items.map(photoTile))];
  });
}

function openPhoto(path) {
  const act = (method, done) => async (e) => {
    const btn = e.currentTarget;
    btn.disabled = true;
    try { done(await api(method, { device: state.deviceId, path })); } catch (err) { toast(String(err), 'error'); } finally { btn.disabled = false; }
  };
  const copyBtn = h('button', { class: 'btn', onclick: act('photo_copy', () => toast('Picture copied')) }, icon('copy', 'sm'), 'Copy');
  const save = h('button', { class: 'btn', onclick: act('photo_save', (r) => toast(`Saved to ${r.path.split('/').slice(-2).join('/')}`)) }, icon('download', 'sm'), 'Save to Downloads');
  openViewer(photoName(path), [copyBtn, save], async () => {
    const r = await api('photo_get', { device: state.deviceId, path });
    return h('img', { src: T.core.convertFileSrc(r.path), alt: photoName(path) });
  }, () => openPhoto(path));
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
  if (view === 'photos' && photos.status !== 'ready') loadPhotos();
  if (view === 'settings') loadSettings();
  if (view === 'files') loadFiles(files.device === state.deviceId ? files.path : '');
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
  apps.timer = setTimeout(pollApps, busy || !st.device ? 1500 : st.windows?.length ? 4000 : 10000);
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
    const r = await api('apps_open', app ? { package: app.package, title: app.name } : { title: 'Phone screen' });
    if (r?.alreadyOpen) toast(app ? `${app.name} is already open in its own window` : 'Your phone screen is already open');
    else if (r?.sharedScreen) toast(`${app.name} is open in the Phone screen window`);
    if (app) {
      apps.recent = [app.package, ...apps.recent.filter((p) => p !== app.package)].slice(0, 6);
      try { localStorage.setItem('tether.recentApps', JSON.stringify(apps.recent)); } catch { /* private mode */ }
    }
  } catch (e) {
    toast(String(e), 'error', 5000);
  }
  apps.opening = null;
  pollApps();
}

async function closeAppWindow(w) {
  try {
    await api('apps_close', w.package ? { package: w.package } : {});
  } catch (e) {
    toast(String(e), 'error');
  }
  pollApps();
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
    st.installHint ? h('div', { class: 'install-hint' },
      h('p', { class: 'note', text: st.canDownload ? 'Or install it from your system\'s packages, in a terminal:' : 'Install it from your system\'s packages, in a terminal:' }),
      h('div', { class: 'row' },
        h('code', { 'data-key': 'apps-install-hint', text: st.installHint }),
        h('button', { class: 'btn', 'data-key': 'apps-install-copy', onclick: () => copy(st.installHint, 'Command copied') }, icon('copy', 'sm'), 'Copy'))) : null,
    st.canDownload
      ? h('div', { class: 'row' },
        h('button', { class: 'btn primary', 'data-key': 'apps-download', disabled: downloading, onclick: () => api('apps_download').then(pollApps).catch((e) => toast(String(e), 'error')) },
          downloading ? h('span', { class: 'spinner sm', 'aria-hidden': 'true' }) : icon('download', 'sm'),
          downloading ? 'Downloading (18 MB)' : 'Download helper (18 MB)'),
        st.installHint ? null : h('span', { class: 'hint', text: 'Or install scrcpy 3.0 or newer yourself.' }))
      : h('div', { class: 'row' },
        h('button', { class: 'btn', 'data-key': 'apps-check', onclick: pollApps }, icon('refresh', 'sm'), 'Check again'),
        h('span', { class: 'hint', text: st.installHint ? 'Then check again.' : 'Install scrcpy 3.0 or newer and adb first.' })));
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
  const isOpen = (apps.status?.windows || []).some((w) => w.package === a.package);
  return h('div', { class: 'app-tile-wrap' },
    h('button', { class: 'app-tile', 'data-key': `app-${a.package}`, title: a.package, disabled: !!apps.opening && !opening, onclick: () => openApp(a) },
      opening ? h('div', { class: 'avatar' }, h('span', { class: 'spinner sm', 'aria-hidden': 'true' })) : appIcon(a),
      h('span', { class: 'name', text: opening ? 'Opening' : a.name }),
      isOpen && !opening ? h('span', { class: 'open-mark', text: 'Open' }) : null),
    // The Flatpak can't write menu entries the desktop sees (see flatpak.rs).
    state.me?.flatpak ? null : h('button', { class: `pin ${inMenu ? 'on' : ''}`, 'data-key': `pin-${a.package}`,
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
        h('div', {}, h('h1', { text: 'Apps' }), h('p', { class: 'sub', text: `From ${device()?.name || st.device.model || 'your phone'}.${state.me?.flatpak ? '' : " Pin an app to add it to this computer's app menu."}` })),
        h('button', { class: 'btn', 'data-key': 'apps-screen', disabled: !!apps.opening, onclick: () => openApp(null) },
          apps.opening === 'screen' ? h('span', { class: 'spinner sm', 'aria-hidden': 'true' }) : icon('phone', 'sm'), 'Phone screen')),
      h('label', { class: 'search' }, icon('search', 'sm'),
        h('input', { type: 'search', 'data-key': 'apps-filter', placeholder: 'Search apps', 'aria-label': 'Search apps', value: apps.filter,
          oninput: (e) => { apps.filter = e.target.value; renderApps(); },
          onkeydown: (e) => { if (e.key === 'Enter' && shown[0]) openApp(shown[0]); } })),
      st.ownWindows === false ? h('p', { class: 'note', 'data-key': 'apps-shared-note' }, icon('alert', 'sm'),
        `Your phone runs Android ${st.device.android || '9 or older'}, so apps open on its own screen, shown in the Phone screen window. With Android 10 or newer each app gets a window of its own.`) : null,
      openWindows(st)),
    body,
  ];
}

function openWindows(st) {
  const list = st.windows || [];
  if (!list.length) return null;
  return h('div', { class: 'open-windows', 'data-key': 'apps-open-windows' },
    h('span', { class: 'label', text: 'Open now' }),
    list.map((w) => h('span', { class: 'chip' }, w.title,
      h('button', { class: 'btn ghost icon-only small', 'data-key': `close-${w.package || 'screen'}`, 'aria-label': `Close ${w.title}`, title: `Close ${w.title}`, onclick: () => closeAppWindow(w) }, icon('x', 'sm')))));
}

// ---------- notifications ----------
function toggleFeed() {
  state.feedOpen = !state.feedOpen;
  savePref('tether.feedOpen', state.feedOpen);
  render();
  if (state.feedOpen) $('.feed button, .feed input')?.focus();
  else $('.nav [data-key="nav-notifications"]')?.focus();
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
        h('button', { class: 'btn ghost icon-only', 'data-key': 'feed-close', 'aria-label': 'Close notifications', title: 'Close notifications', onclick: toggleFeed }, icon('x')))),
    h('div', { class: 'feed-body' },
      code ? h('div', { class: 'code-card' },
        h('div', { class: 'from' }, icon('key', 'sm'), `Code from ${code.from}`),
        h('div', { class: 'row' }, h('span', { class: 'digits', text: code.code }),
          h('button', { class: 'btn primary', onclick: () => copy(code.code, 'Code copied') }, icon('copy', 'sm'), 'Copy'))) : null,
      receivedPanel(),
      notes.length ? notes.map(noteCard) : h('div', { class: 'empty' }, icon('bell'), h('p', { text: 'No notifications on your phone.' }))),
  ]);
}

function fileSize(n) {
  if (!(n >= 0)) return '';
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB'];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

function transferRow(t) {
  const files = t.number_of_files > 1 ? ` · ${t.number_of_files} files` : '';
  const row = h('article', { class: 'transfer', 'aria-label': t.name },
    h('div', { class: 'top' }, icon(t.sending ? 'upload' : 'download', 'sm'), h('span', { class: 'name', title: t.name, text: t.name })));
  if (t.error) {
    row.append(h('div', { class: 'status error', role: 'alert', text: t.error }));
  } else if (t.path) {
    row.append(h('div', { class: 'acts' },
      h('button', { class: 'btn', onclick: () => api('received_open', { path: t.path }).catch((e) => toast(String(e), 'error')) }, 'Open'),
      h('button', { class: 'btn ghost', onclick: () => api('received_show', { path: t.path }).catch((e) => toast(String(e), 'error')) }, 'Show in folder')));
  } else if (t.done) {
    row.append(h('div', { class: 'status', text: `Sent${files}` }));
  } else {
    const known = t.total > 0;
    row.append(
      h('div', { class: 'status', text: `${t.sending ? 'Sending' : 'Receiving'}${known ? ` · ${fileSize(t.bytes)} of ${fileSize(t.total)}` : ''}${files}` }),
      h('progress', { max: Math.max(1, t.total), ...(known ? { value: t.bytes } : {}), 'aria-label': `${t.sending ? 'Sending' : 'Receiving'} ${t.name}` }));
  }
  return row;
}

function receivedPanel() {
  // Only once something arrived: the notifications column stays about the phone until then.
  if (!state.transfers.length) return null;
  return h('section', { class: 'received-panel', 'aria-label': 'Received from your phone' },
    h('h3', { text: 'Received from your phone' }),
    state.transfers.slice(0, 20).map(transferRow));
}
function transferProgress(u) {
  if (!u.name) {
    for (const t of state.transfers) if (!t.sending && !t.path && !t.error) Object.assign(t, { number_of_files: u.number_of_files, total_payload_size: u.total_payload_size });
    invalidate('feed'); return;
  }
  let t = state.transfers.find((t) => t.name === u.name && t.sending === u.sending && !t.path && !t.done && !t.error);
  if (!t) { t = {}; state.transfers.unshift(t); }
  Object.assign(t, u, { done: u.sending && u.total >= 0 && u.bytes >= u.total });
  state.transfers = state.transfers.slice(0, 100);
  invalidate('feed');
}
function sendTextOrLink() {
  const root = $('#palette-root');
  const text = h('textarea', { 'aria-label': 'Text or link', rows: 5, required: true });
  const send = h('button', { class: 'btn primary', type: 'submit' }, 'Send');
  root.replaceChildren(h('div', { class: 'palette-backdrop', onclick: (e) => { if (e.target === e.currentTarget) root.replaceChildren(); } },
    h('form', { class: 'palette', role: 'dialog', 'aria-label': 'Send text or link', onsubmit: async (e) => {
      e.preventDefault(); const value = text.value.trim(); if (!value) return; send.disabled = true;
      const isUrl = /^https?:\/\/\S+$/i.test(value);
      try { await api(isUrl ? 'share_url' : 'share_text', { device: state.deviceId, [isUrl ? 'url' : 'text']: value }); root.replaceChildren(); toast('Sent to your phone'); }
      catch (e) { toast(String(e), 'error'); send.disabled = false; }
    } }, h('h2', { text: 'Send text or link' }), text, send, h('button', { class: 'btn ghost', type: 'button', onclick: () => root.replaceChildren() }, 'Cancel'))));
  text.focus();
}

function letterBadge(app) {
  let hash = 0;
  for (const ch of app || '?') hash = (hash * 31 + ch.codePointAt(0)) >>> 0;
  return h('span', { class: 'app-badge', 'aria-hidden': 'true', style: `background:hsl(${hash % 360} 45% 46%)`, text: (app || '?')[0].toUpperCase() });
}

// The app's icon from the phone, or its first letter until the icon has arrived (or if the
// cached file was pruned or can't be decoded).
function appBadge(n) {
  if (!n.icon_path) return letterBadge(n.app);
  const img = h('img', { class: 'app-icon-img', src: T.core.convertFileSrc(n.icon_path), alt: '' });
  img.addEventListener('error', () => img.replaceWith(letterBadge(n.app)), { once: true });
  return img;
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
  if (isScreenshot(n)) {
    acts.push(h('button', { class: 'btn', onclick: (e) => screenshot(e.currentTarget, 'copy') }, icon('copy', 'sm'), 'Copy'));
    acts.push(h('button', { class: 'btn', onclick: (e) => screenshot(e.currentTarget, 'save') }, icon('download', 'sm'), 'Save'));
  }
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

// Same test as the service's desktop popup (phonefiles::is_screenshot).
function isScreenshot(n) {
  return [n.app, n.title, n.text].some((s) => (s || '').toLowerCase().includes('screenshot'));
}

async function screenshot(btn, mode) {
  const busy = toast(mode === 'copy' ? 'Copying the screenshot from your phone' : 'Saving the screenshot from your phone', 'progress', 90000);
  btn.disabled = true;
  try {
    if (mode === 'copy') {
      await api('screenshot_copy', { device: state.deviceId });
      toast('Screenshot copied');
    } else {
      const r = await api('screenshot_save', { device: state.deviceId });
      toast(`Saved to ${r.path.split('/').slice(-2).join('/')}`);
    }
  } catch (e) { toast(String(e), 'error'); } finally { busy.remove(); btn.disabled = false; }
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
    requested_by_us: [h('span', { class: 'verify', text: d.verification_key || '' }), h('div', { class: 'spinner', title: 'Waiting for the phone' }),
      h('button', { class: 'btn', onclick: () => pairAction('reject', d.id) }, 'Cancel')],
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
    live && { label: 'Send text or link', icon: 'send', run: sendTextOrLink },
    live && { label: 'Ring my phone', icon: 'ring', run: ringPhone },
    live && { label: 'Ping my phone', icon: 'bolt', run: () => quick('ping', 'Ping sent') },
    live && player && { label: player.is_playing ? `Pause ${player.title}` : `Play ${player.title}`, sub: player.name, icon: player.is_playing ? 'pause' : 'play', run: () => mediaAction(player, 'PlayPause') },
    live && player && player.can_go_next && { label: 'Next song', sub: player.name, icon: 'next', run: () => mediaAction(player, 'Next') },
    state.call && { label: 'Mute the ringing call', icon: 'mute', run: () => quick('mute', 'Ringer muted') },
    { label: 'Look for my phone', icon: 'refresh', run: () => quick('refresh', 'Looking for your phone', true) },
    { label: state.feedOpen ? 'Hide notifications' : 'Show notifications', icon: 'bell', run: toggleFeed },
    { label: 'Check for updates', icon: 'download', run: checkForUpdates },
    recentCode() && { label: `Copy code ${recentCode().code}`, sub: recentCode().from, icon: 'key', run: () => copy(recentCode().code, 'Code copied') },
  ].filter(Boolean);
  cmds.unshift(...activeCallCommands());
  cmds.push({ label: 'Show phone apps', icon: 'apps', run: () => showView('apps') });
  cmds.push({ label: 'Show phone photos', icon: 'image', run: () => showView('photos') });
  // After the everyday commands: the palette shows nine before anything is typed.
  if (canTakePhoto(d)) cmds.push({ label: 'Take a photo', sub: 'With the phone\'s camera', icon: 'camera', run: takePhoto });
  for (const [view, label] of [['files', 'Browse phone files'], ['contacts', 'Show contacts'], ['keyboard', 'Keyboard'], ['devices', 'Manage phones']])
    cmds.push({ label, icon: view === 'keyboard' ? 'key' : 'phone', run: () => showView(view) });
  cmds.push({ label: 'Settings', icon: 'settings', run: () => showView('settings') });
  cmds.push({ label: 'Quit Tether', sub: 'Stops it until you open it again', icon: 'x', run: quitTether });
  const quietCmd = quietCommand(); if (quietCmd) cmds.push(quietCmd);
  if (apps.status?.device) {
    cmds.push({ label: 'Show phone screen', icon: 'phone', run: () => openApp(null) });
    for (const a of apps.list || []) cmds.push({ label: `Open ${a.name}`, sub: 'Phone app', icon: 'apps', run: () => openApp(a) });
  }
  for (const m of heads()) cmds.push({ label: threadTitle(m.addresses), sub: 'Conversation', icon: 'message', run: () => selectThread(m.thread_id) });
  for (const c of state.contacts) cmds.push({ label: `Text ${c.name}`, sub: prettyNumber(c.number), icon: 'send', run: () => openNew(c) });
  cmds.push(...dialCommands());
  return cmds;
}

function openPalette() {
  const root = $('#palette-root');
  if (root.firstChild) return;
  let sel = 0, shown = [];
  // An ARIA combobox: focus stays in the field and the highlighted command is announced through
  // aria-activedescendant, so screen readers follow the arrow keys.
  const ul = h('ul', { id: 'palette-list', role: 'listbox', 'aria-label': 'Commands' });
  const count = h('div', { class: 'sr-only', role: 'status', 'aria-live': 'polite' });
  const move = (to) => { if (shown.length) { sel = Math.max(0, Math.min(shown.length - 1, to)); draw(); } };
  const input = h('input', { placeholder: 'Type a command, a name or a conversation', 'aria-label': 'Command', role: 'combobox',
    'aria-expanded': 'true', 'aria-controls': 'palette-list', 'aria-autocomplete': 'list', autocomplete: 'off', spellcheck: 'false',
    oninput: () => { sel = 0; draw(); },
    onkeydown: (e) => {
      if (e.key === 'Escape') close();
      else if (e.key === 'ArrowDown') { e.preventDefault(); move(sel + 1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); move(sel - 1); }
      else if ((e.key === 'Home' || e.key === 'End') && e.ctrlKey) { e.preventDefault(); move(e.key === 'Home' ? 0 : shown.length - 1); }
      else if (e.key === 'Enter' && shown[sel]) { e.preventDefault(); close(); shown[sel].run(); }
      // Nothing else in the dialog takes focus; keep it here rather than behind the backdrop.
      else if (e.key === 'Tab') e.preventDefault();
    } });
  const close = () => { root.replaceChildren(); };
  let lastCount = -1;
  const draw = () => {
    const q = input.value.trim().toLowerCase();
    shown = commands().filter((c) => !q || `${c.label} ${c.sub || ''}`.toLowerCase().includes(q)).slice(0, 9);
    ul.replaceChildren(...(shown.length ? shown.map((c, i) => h('li', { id: `palette-opt-${i}`, role: 'option', 'aria-selected': i === sel ? 'true' : 'false',
      onmousemove: () => { if (sel !== i) { sel = i; draw(); } }, onclick: () => { close(); c.run(); } },
    icon(c.icon), h('span', { text: c.label }), c.sub ? h('span', { class: 'sub', text: c.sub }) : null)) : [h('li', { class: 'none', role: 'presentation', text: 'No matches' })]));
    if (shown.length) input.setAttribute('aria-activedescendant', `palette-opt-${sel}`);
    else input.removeAttribute('aria-activedescendant');
    // Announce the count only when it changes, not on every arrow key.
    if (shown.length !== lastCount) {
      lastCount = shown.length;
      count.textContent = !shown.length ? 'No matches' : shown.length === 1 ? '1 command' : `${shown.length} commands`;
    }
  };
  root.replaceChildren(h('div', { class: 'palette-backdrop', onclick: (e) => { if (e.target === e.currentTarget) close(); } },
    h('div', { class: 'palette', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Commands' }, h('label', { class: 'search' }, icon('search', 'sm'), input), ul, count)));
  draw();
  input.focus();
}

document.addEventListener('keydown', (e) => {
  const mod = e.ctrlKey || e.metaKey;
  if (mod && (e.key === 'k' || e.key === 'p')) { e.preventDefault(); openPalette(); }
  else if (mod && e.key === 'n' && state.deviceId) { e.preventDefault(); openNew(); }
  else if (e.key === 'Escape' && state.feedOpen && window.innerWidth <= 1180) toggleFeed();
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
      $('#drop-text').textContent = !(d && d.connected) ? 'Your phone is out of reach'
        : state.view === 'files' ? (files.path ? `Drop to upload to ${files.path.split('/').pop() || 'this folder'}` : 'Open a folder to upload into') : `Drop to send to ${d.name}`;
      overlay.hidden = false;
    } else if (t === 'leave') {
      overlay.hidden = true;
    } else if (t === 'drop') {
      overlay.hidden = true;
      if (!(d && d.connected)) return toast('Your phone is out of reach', 'error');
      if (state.view === 'files') { for (const path of e.payload.paths || []) await uploadFile(path); } else sendFiles(e.payload.paths || [], d);
    }
  });
}

// "Send files..." in the tray menu.
async function pickAndSendFiles() {
  const d = device();
  if (!(d && d.connected)) return toast('Your phone is out of reach', 'error');
  let paths;
  try { paths = await T.core.invoke('choose_files', { title: `Send to ${d.name}` }); } catch (e) { return toast(`The file picker didn't open: ${e}`, 'error'); }
  sendFiles(paths, d);
}

async function sendFiles(paths, d) {
  if (!paths.length) return;
  try { await api('share_files', { device: d.id, paths }); toast(`${paths.length} ${paths.length === 1 ? 'file sent' : 'files sent'} to ${d.name}`); }
  catch (e) { toast(`Files didn't send: ${e}`, 'error'); }
}

// A finished download from the phone (a shared file or a camera photo) completes its progress row.
function receivedFile(path) {
  const name = path.split('/').pop();
  const t = state.transfers.find((t) => !t.sending && !t.path && !t.error && (t.name === name || name.startsWith(t.name.replace(/(\.[^.]+)?$/, ' ('))));
  if (t) t.path = path; else state.transfers.unshift({ name, path });
  invalidate('feed');
}

// ---------- events ----------
function onEvent(ev) {
  if (ev.event === 'computer_settings') { openCommandsSettings(); return; }
  if (ev.event === 'send_files') { pickAndSendFiles(); return; }
  if (ev.event === 'scheduled') { scheduled.items = ev.items || []; invalidate('thread'); return; }
  if (ev.event === 'show_notifications') { showNotifications(); return; }
  if (ev.event === 'settings') { loadSettings(); return; }
  if (ev.event === 'computer_ring') { state.computerRinging = ev.ringing; renderRail(); return; }
  if (ev.event === 'calls') { onCalls(ev); return; }
  if (ev.event === 'device') {
    const i = state.devices.findIndex((d) => d.id === ev.id);
    const before = i >= 0 ? state.devices[i] : null;
    if (i >= 0) state.devices[i] = ev; else state.devices.push(ev);
    if (!ev.connected && ev.pair === 'unpaired') state.devices = state.devices.filter((d) => d.id !== ev.id);
    const becamePaired = ev.pair === 'paired' && (!before || before.pair !== 'paired');
    const cameBack = ev.id === state.deviceId && ev.connected && before && !before.connected;
    const lost = ev.id === state.deviceId && ev.pair !== 'paired';
    if (lost) { state.deviceId = state.devices.find((x) => x.pair === 'paired')?.id || null; state.threads.clear(); state.notifications = {}; photos.status = 'idle'; photos.folders = []; photos.thumbs.clear(); photos.tiles.clear(); }
    if (lost && state.deviceId) loadDevice().then(render);
    if (becamePaired && !state.deviceId) { state.deviceId = ev.id; loadDevice().then(render); }
    else if (cameBack) loadDevice().then(render);
    if (!state.deviceId || !$('.rail')) return render();
    renderRail(); renderBanners(); if (['devices', 'keyboard'].includes(state.view)) renderTools();
    if (cameBack || !ev.connected) renderThreadPane(true);
    return;
  }
  if (ev.event !== 'update' || ev.device_id !== state.deviceId) return;
  const u = ev.update;
  switch (u.kind) {
    case 'keyboard_state': state.keyboardActive = u.active; if (state.view === 'keyboard') renderTools(); break;
    case 'keyboard_echo': if ($('#keyboard-status')) $('#keyboard-status').textContent = 'Received by your phone'; break;
    case 'battery': state.battery = u; renderRail(); break;
    case 'attachment': waitingAttachments.get(u.unique_identifier)?.resolve(u.path); waitingAttachments.delete(u.unique_identifier); break;
    case 'media': state.media = Object.fromEntries(u.players.map((p) => [p.name, p])); renderRail(); break;
    case 'signal': state.signal = u.signal; renderRail(); break;
    case 'notification': state.notifications[u.id] = u; invalidate('feed', 'rail'); break;
    case 'notification_removed': delete state.notifications[u.id]; invalidate('feed', 'rail'); break;
    case 'messages':
      mergeMessages(u.messages);
      if (u.messages.some((m) => m.thread_id === state.selected)) {
        state.loadingThread = null;
        const msgs = threadMessages(state.selected);
        state.seen[state.selected] = msgs[msgs.length - 1].date; saveSeen();
        invalidate('thread');
      }
      // A confirmed text of ours removes its "Sending" bubble, wherever it was.
      if (u.messages.some((m) => m.type === 2)) invalidate('thread');
      invalidate('list', 'rail', 'feed');
      break;
    case 'contacts':
      api('contacts', { device: state.deviceId }).then((c) => { state.contacts = c; invalidate('list', 'thread'); if (state.view === 'contacts') renderTools(); }).catch(() => {});
      break;
    case 'call': state.call = u.state === 'missed' ? null : u; renderRail(); break;
    case 'call_ended': state.call = null; renderRail(); break;
    case 'ping': toast(u.message ? `Phone: ${u.message}` : 'Ping from your phone'); break;
    case 'clipboard': phoneClipboard(u.content); break;
    case 'shared_text': toast('Text from your phone copied'); break;
    case 'shared_url': toast('Opening the link from your phone'); break;
    case 'transfer_progress': transferProgress(u); break;
    case 'photo_taken': {
      receivedFile(u.path);
      // Closing the viewer doesn't stop the phone; a picture that arrives later still gets a word.
      const showing = photoWait && $('.viewer');
      photoWait?.resolve(u.path);
      if (!showing) toast(`Photo from your phone saved as ${u.path.split('/').pop()}`);
      break;
    }
    case 'photo_cancelled': photoWait?.reject(new Error('The camera closed without a photo.')); break;
    case 'file_received': receivedFile(u.path); toast(`Received ${u.path.split('/').pop()}`); break;
    case 'transfer_failed': { const t = state.transfers.find((t) => t.name === u.name && !t.path && !t.done); if (t) t.error = u.reason; invalidate('feed'); } toast(`${u.name} didn't arrive: ${u.reason}`, 'error'); break;
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

// ---------- Phone tools ----------
const files = { device: null, path: '', roots: [], entries: [], loading: false, error: '', sort: 'name', reverse: false, generation: 0 };
let contactsQuery = '';
function toolButton(text, run, extra = {}) { return h('button', { class: 'btn', onclick: run, ...extra }, text); }
function toolHead(title, sub, ...actions) {
  return h('header', { class: 'apps-head' }, h('div', { class: 'row' }, h('div', {}, h('h1', { text: title }), h('p', { class: 'sub', text: sub })), ...actions));
}
async function loadFiles(path = '') {
  const generation = ++files.generation, id = state.deviceId;
  files.device = id; files.path = path; files.loading = true; files.error = ''; renderTools();
  try {
    const result = await api('files_list', { device: id, path });
    if (generation !== files.generation || state.deviceId !== id) return;
    files.roots = result.roots; files.entries = result.entries;
  } catch (e) { if (generation === files.generation) files.error = String(e); }
  finally { if (generation === files.generation) { files.loading = false; renderTools(); } }
}
// Native dialogs contain focus and Escape; return focus to the opener on close.
function askFile(title, initial, run, destructive = false) {
  const previous = document.activeElement;
  const input = destructive ? null : h('input', { value: initial, 'aria-label': title, required: true });
  const dialog = h('dialog', { class: 'file-dialog', 'aria-label': title },
    h('form', { onsubmit: async (e) => {
      e.preventDefault();
      const value = input?.value.trim();
      if (input && (!value || value.includes('/') || value === '.' || value === '..')) { input.setCustomValidity('Use a name without slashes.'); input.reportValidity(); return; }
      dialog.close(); try { await run(value); } catch (err) { toast(String(err), 'error'); }
    } }, h('h2', { text: title }), input,
    destructive ? h('p', { text: 'This removes it from your phone. Folders must be empty.' }) : null,
    h('div', { class: 'row' }, h('button', { type: 'button', class: 'btn', onclick: () => dialog.close() }, 'Cancel'),
      h('button', { class: destructive ? 'btn danger' : 'btn primary', type: 'submit' }, destructive ? 'Delete' : 'Save'))));
  dialog.addEventListener('close', () => { dialog.remove(); if (previous?.isConnected) previous.focus(); });
  input?.addEventListener('input', () => input.setCustomValidity(''));
  document.body.append(dialog); dialog.showModal(); (input || $('button', dialog)).focus(); input?.select();
}
async function fileAction(action, entry, target) {
  const busy = toast(action === 'save' ? 'Saving to Downloads' : 'Working on your phone', 'progress', 90000);
  try {
    const r = await api('file_operation', { device: state.deviceId, action, path: entry.path, directory: entry.directory, target });
    if (action === 'save') toast(`Saved to ${r.path}`);
    else if (action !== 'open') await loadFiles(files.path);
  } catch (e) { toast(String(e), 'error'); } finally { busy.remove(); }
}
async function uploadFile(path) {
  if (!files.path) return toast('Open a folder before uploading', 'error');
  await fileAction('upload', { path: `${files.path.replace(/\/$/, '')}/${path.split('/').pop()}` }, path);
}
async function chooseUpload() {
  try { for (const path of await T.core.invoke('choose_files')) await uploadFile(path); }
  catch (e) { toast(String(e), 'error'); }
}
// Setup copy comes from the same capability mapping used by the daemon API.
function phoneSetup(feature, d = device()) {
  const item = d?.readiness?.find((f) => f.id === feature);
  return item ? `KDE Connect > ${state.me?.deviceName || 'this computer'} > Plugin settings > ${item.instruction}` : '';
}
// What the phone has actually sent so far. Android lists every plugin it has whether it is on or
// not (PluginFactory.kt), so only data that arrived proves a feature works.
const SEEN = {
  texts: () => state.threads.size > 0,
  notifications: () => Object.keys(state.notifications || {}).length > 0,
  contacts: () => state.contacts.length > 0,
  battery: () => !!state.battery,
  connectivity: () => Object.keys(state.signal || {}).length > 0,
  mpris: () => Object.keys(state.media || {}).length > 0,
  keyboard: () => state.keyboardActive === true,
};
function featureStatus(f, d = device()) {
  if (!f.ready) return 'missing';
  const seen = SEEN[f.id];
  // The window only holds data for the phone that is open; for another phone there's no proof either way.
  if (!seen || !d || d.id !== state.deviceId) return 'unknown';
  return seen() ? 'working' : 'waiting';
}
const STATUS_TEXT = { working: 'Working', waiting: 'Nothing yet', missing: 'Not on this phone', unknown: '' };
function readinessHint(feature) {
  const d = device();
  const item = d?.readiness?.find((f) => f.id === feature);
  if (!item || !d.connected) return null;
  const status = featureStatus(item, d);
  if (status === 'missing') return h('p', { class: 'readiness-hint', text: 'Your version of KDE Connect on the phone does not have this. Updating the app may add it.' });
  return status === 'waiting' ? h('p', { class: 'readiness-hint', text: `If nothing comes, check this on the phone: ${phoneSetup(feature, d)}` }) : null;
}
// Leads with what needs doing; the full list (with every step) stays one click away.
function phoneChecklist(d) {
  const items = (d.readiness || []).map((f) => ({ ...f, status: featureStatus(f, d) }));
  const working = items.filter((f) => f.status === 'working');
  const waiting = items.filter((f) => f.status === 'waiting');
  const missing = items.filter((f) => f.status === 'missing');
  const fix = (f) => h('details', { class: 'fix', 'data-feature': f.id },
    h('summary', {}, icon('alert', 'sm'), h('b', { text: f.label }), h('span', { class: 'readiness-status', text: 'How to turn it on' }), icon('chevron', 'sm chev')),
    h('p', { text: phoneSetup(f.id, d) }));
  const summary = [working.length && `${working.length} working`, waiting.length && `${waiting.length} to set up`].filter(Boolean).join(' · ');
  return h('section', { class: 'phone-readiness', 'aria-label': 'On your phone' },
    h('div', { class: 'readiness-head' }, h('h2', { text: 'Phone features' }), summary ? h('span', { class: 'readiness-summary', text: summary }) : null),
    !d.connected ? h('p', { class: 'sub', text: 'Your phone is offline, so this shows what it had last time.' }) : null,
    working.length ? h('ul', { class: 'feature-chips', 'aria-label': 'Working' }, working.map((f) => h('li', {}, icon('check', 'sm'), f.label))) : null,
    waiting.length ? h('div', { class: 'fix-list' },
      h('p', { class: 'sub', text: waiting.length === 1 ? 'One feature has sent nothing yet. Its switch may be off in KDE Connect on the phone.' : `${waiting.length} features have sent nothing yet. Their switches may be off in KDE Connect on the phone.` }),
      waiting.map(fix)) : null,
    missing.length ? h('p', { class: 'sub', text: `Not in this version of KDE Connect on the phone: ${missing.map((f) => f.label).join(', ')}.` }) : null,
    h('details', { class: 'all-features' },
      h('summary', {}, icon('chevron', 'sm chev'), `Show all ${items.length} features and their switches`),
      h('ul', { class: 'readiness-list' }, items.map((f) => h('li', { 'data-feature': f.id, 'data-status': f.status, 'data-ready': String(f.status === 'working') },
        h('div', { class: 'row' }, icon(f.status === 'working' ? 'check' : f.status === 'unknown' ? 'dot' : 'alert', 'sm'), h('b', { text: f.label }),
          STATUS_TEXT[f.status] ? h('span', { class: 'readiness-status', text: STATUS_TEXT[f.status] }) : null),
        f.status === 'working' ? null : h('p', { text: f.status === 'missing' ? 'Not in this version of KDE Connect on the phone.' : phoneSetup(f.id, d) })))),
      h('p', { class: 'sub', text: 'Each feature needs its switch on in the KDE Connect app, and some need a permission.' }),
      h('p', { class: 'sub', text: 'On Samsung phones, also set Settings > Apps > KDE Connect > Battery > Unrestricted, so the phone keeps the connection while it sleeps.' })));
}

function renderTools() {
  const page = $('.tools-page');
  if (!page || !['files', 'contacts', 'keyboard', 'devices'].includes(state.view)) return;
  if (state.view === 'files') return renderFiles(page);
  if (state.view === 'contacts') {
    const query = contactsQuery.toLowerCase(), groups = new Map();
    for (const c of state.contacts) {
      if (!`${c.name} ${c.number}`.toLowerCase().includes(query)) continue;
      if (!groups.has(c.name)) groups.set(c.name, []);
      groups.get(c.name).push(c);
    }
    swap(page, () => [toolHead('Contacts', 'Contacts shared by your phone.'),
      h('div', { class: 'tools-body' },
        h('label', { class: 'search' }, icon('search', 'sm'), h('input', { type: 'search', 'data-key': 'contacts-search', 'aria-label': 'Search contacts', placeholder: 'Search names or numbers', value: contactsQuery, oninput: (e) => { contactsQuery = e.target.value; renderTools(); } })),
        !groups.size ? h('div', { class: 'empty' }, icon('user'), h('p', { text: query ? 'No matching contacts.' : 'No contacts shared yet.' }),
          !query ? (readinessHint('contacts') || h('p', { text: phoneSetup('contacts') })) : null) :
          [...groups].map(([name, numbers]) => h('article', { class: 'contact-card' }, h('h2', { text: name }), numbers.map((c) =>
            h('div', { class: 'row' }, h('span', { class: 'grow', text: prettyNumber(c.number) }), toolButton('Text', () => openNew(c)), toolButton('Call', () => dialNumber(c.number, c.name)), toolButton('Copy number', () => copy(c.number, 'Number copied'))))))) ]);
  } else if (state.view === 'keyboard') {
    swap(page, () => [toolHead('Keyboard', 'Type into the text field selected on your phone.'),
      h('div', { class: 'tools-body keyboard-body' }, readinessHint('keyboard'),
        h('p', { class: 'banner', text: state.keyboardActive ? 'Your phone is ready. Select a text field on it, then type below.' : 'Pick KDE Connect Remote Keyboard as the keyboard on your phone.' }),
        !state.keyboardActive ? h('ol', { class: 'steps' }, h('li', { text: 'On the phone, open KDE Connect → this computer → Plugin settings → Remote keyboard and enable it.' }), h('li', { text: 'In Android keyboard settings, enable KDE Connect Remote Keyboard. Open a text field and pick it from the keyboard switcher.' })) : null,
        h('label', { for: 'keyboard-box', text: 'Type here to send keys' }),
        h('textarea', { id: 'keyboard-box', 'data-key': 'keyboard-box', placeholder: 'Click here and type', spellcheck: 'false', 'aria-describedby': 'keyboard-status',
          onkeydown: keyboardKey,
          oninput: (e) => { if (!e.isComposing && e.target.value) { sendKey({ key: e.target.value }); e.target.value = ''; } },
          oncompositionend: (e) => { if (e.data) sendKey({ key: e.data }); e.target.value = ''; },
          onpaste: (e) => { e.preventDefault(); const text = e.clipboardData.getData('text'); if (text) sendKey({ key: text }); } }),
        h('p', { id: 'keyboard-status', role: 'status', text: device()?.connected ? 'Keys are sent only while this box has focus. Tab leaves the box.' : 'Your phone is offline.' }),
        h('div', { class: 'row' }, toolButton('Send Tab', () => sendKey({ specialKey: 2 })), toolButton('Send Enter', () => sendKey({ specialKey: 12 })), toolButton('Send Escape', () => sendKey({ specialKey: 14 }))))]);
  } else {
    swap(page, () => [toolHead('Devices', 'Pair, switch or forget a phone.', toolButton('Connect by address', connectByAddress)),
      h('div', { class: 'tools-body' }, state.devices.map((d) => d.pair === 'paired' ? h('article', { class: 'contact-card device-card' },
        h('div', { class: 'device-head' },
          h('div', { class: 'device-title' }, icon('phone'),
            h('div', {}, h('h2', { text: d.name }),
              h('p', {}, h('span', { class: `dot ${d.connected ? 'on' : 'wait'}` }),
                [d.connected ? `Connected${d.address ? ` · ${d.address}` : ''}` : (d.last_seen ? `Last seen ${ago(d.last_seen)} · Reconnecting` : 'Offline · Tether keeps reconnecting'), phoneFacts(d)].filter(Boolean).join(' · ')))),
          h('div', { class: 'row' }, d.id === state.deviceId ? null : toolButton('Use this phone', () => switchDevice(d.id)),
            toolButton('Forget phone', () => askFile(`Forget ${d.name}?`, '', () => pairAction('unpair', d.id), true)))),
        phoneChecklist(d)) : pairRow(d)),
      toolButton('Look for phones', () => quick('refresh', 'Looking for phones', true)))]);
  }
}
function renderFiles(page) {
  const entries = [...files.entries].sort((a, b) => Number(b.directory) - Number(a.directory) ||
    (files.reverse ? -1 : 1) * (files.sort === 'size' ? a.size - b.size : files.sort === 'date' ? fileDate(a.date) - fileDate(b.date) : a.name.localeCompare(b.name, undefined, { numeric: true })));
  const root = files.roots.find((r) => files.path === r || files.path.startsWith(r.replace(/\/$/, '') + '/')) || '';
  const parts = files.path.slice(root.length).split('/').filter(Boolean);
  swap(page, () => [toolHead('Files', 'Browse your phone. Drop files here to upload into the open folder.',
    toolButton('Refresh', () => loadFiles(files.path), { disabled: files.loading })),
    h('div', { class: 'tools-body' }, readinessHint('sftp'),
      h('nav', { class: 'breadcrumbs', 'aria-label': 'Folder path' }, toolButton('Storage', () => loadFiles('')),
        root ? toolButton('Internal storage', () => loadFiles(root)) : null,
        parts.map((part, i) => toolButton(part, () => loadFiles(root.replace(/\/$/, '') + '/' + parts.slice(0, i + 1).join('/'))))),
      h('div', { class: 'row file-toolbar' }, toolButton('Upload files', chooseUpload, { disabled: !files.path || files.loading }),
        toolButton('New folder', () => askFile('New folder', '', (name) => fileAction('mkdir', { path: `${files.path}/${name}` })), { disabled: !files.path || files.loading }),
        h('label', {}, 'Sort ', h('select', { 'aria-label': 'Sort files', onchange: (e) => { files.sort = e.target.value; renderTools(); } },
          ['name', 'size', 'date'].map((v) => h('option', { value: v, selected: files.sort === v, text: v[0].toUpperCase() + v.slice(1) })))),
        toolButton(files.reverse ? 'Descending' : 'Ascending', () => { files.reverse = !files.reverse; renderTools(); })),
      files.loading ? h('div', { class: 'empty', role: 'status' }, h('div', { class: 'spinner' }), 'Getting files from your phone') :
      files.error ? h('div', { class: 'empty', role: 'alert' }, icon('alert'), h('p', { text: files.error }), h('p', { text: phoneSetup('sftp') }), toolButton('Try again', () => loadFiles(files.path))) :
      !files.path ? h('div', { class: 'storage-roots' }, files.roots.length ? files.roots.map((root, i) => toolButton(files.roots.length === 1 ? 'Internal storage' : `Storage ${i + 1}`, () => loadFiles(root))) : h('p', { text: 'No storage shared. Add a storage location in KDE Connect → Filesystem expose on your phone.' })) :
      !entries.length ? h('div', { class: 'empty' }, icon('folder'), h('p', { text: 'This folder is empty. Upload a file or create a folder.' })) :
      h('div', { class: 'file-list' }, entries.map((entry) => h('article', { class: 'file-row' },
        h('div', { class: 'file-info' }, h('button', { class: 'file-name', onclick: () => entry.directory ? loadFiles(entry.path) : fileAction('open', entry), text: entry.name }),
          h('small', { text: `${entry.directory ? 'Folder' : formatFileSize(entry.size)} · ${entry.date}` })),
        h('div', { class: 'file-actions' }, !entry.directory ? toolButton('Save', () => fileAction('save', entry)) : null,
          toolButton('Rename', () => askFile('Rename', entry.name, (name) => fileAction('rename', entry, `${files.path}/${name}`))),
          toolButton('Delete', () => askFile(`Delete ${entry.name}?`, '', () => fileAction('delete', entry), true))))))) ]);
}
function fileDate(value) {
  const parts = value.split(' '), now = new Date();
  const timed = parts[2]?.includes(':');
  const date = new Date(timed ? value + ' ' + now.getFullYear() : value);
  if (timed && date > now) date.setFullYear(now.getFullYear() - 1);
  return date.getTime() || 0;
}
function formatFileSize(bytes) {
  return bytes < 1024 ? `${bytes} B` : bytes < 1048576 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1048576).toFixed(1)} MB`;
}
let keyboardQueue = Promise.resolve();
function sendKey(params) {
  if (!device()?.connected) return toast('Your phone is offline', 'error');
  if ($('#keyboard-status')) $('#keyboard-status').textContent = 'Sending to your phone…';
  const id = state.deviceId;
  keyboardQueue = keyboardQueue.then(() => api('keyboard', { device: id, ...params })).catch((e) => toast(String(e), 'error'));
}
function keyboardKey(e) {
  if (e.isComposing || e.key === 'Tab') return;
  const special = { Backspace: 1, ArrowLeft: 4, ArrowUp: 5, ArrowRight: 6, ArrowDown: 7, PageUp: 8, PageDown: 9, Home: 10, End: 11, Enter: 12, Delete: 13, Escape: 14 };
  const specialKey = special[e.key] || (/^F([1-9]|1[0-2])$/.test(e.key) ? 20 + Number(e.key.slice(1)) : null);
  if (specialKey || ((e.ctrlKey || e.altKey || e.metaKey) && e.key.length === 1)) {
    e.preventDefault(); e.stopPropagation();
    sendKey({ key: specialKey ? '' : e.key, specialKey, shift: e.shiftKey, ctrl: e.ctrlKey, alt: e.altKey, super: e.metaKey });
  }
}
async function switchDevice(id) {
  if (id === state.deviceId) return;
  state.deviceId = id; state.threads.clear(); state.selected = null; state.pending = [];
  state.battery = null; state.contacts = []; state.notifications = {}; state.keyboardActive = null;
  photos.status = 'idle'; photos.folders = []; photos.thumbs.clear(); photos.tiles.clear();
  files.generation++; files.device = null; files.entries = []; files.path = '';
  await loadDevice(); render(); if (state.view === 'files') loadFiles();
}

// ---------- Calls through this computer (Bluetooth hands-free, crates/tether/src/calls.rs) ----------
// The daemon pushes `calls` events with {blocker, phone, audio_here, can_mute, muted, calls}; the
// banner floats over every view so a ringing call can be answered from anywhere.
const calls = { status: null, busy: null, timer: null, muting: false, keypad: null, keyed: '', tones: Promise.resolve() };

const callSetup = {
  service: { why: 'Calling from this computer needs PipeWire 1.4 or newer with its Bluetooth support.',
    steps: ['Update your system. Ubuntu 25.04, Fedora 42 and newer already have it.', 'If it still doesn\'t work, install the PipeWire Bluetooth package (often called pipewire-bluetooth or libspa-0.2-bluetooth), then log out and back in.'] },
  bluetooth: { why: 'Bluetooth isn\'t running on this computer, or it has no Bluetooth.',
    steps: ['Turn on Bluetooth in your desktop\'s settings.', 'No Bluetooth on this computer? A small USB Bluetooth adapter works.'] },
  adapter_off: { why: 'Bluetooth is off on this computer.', steps: ['Turn on Bluetooth in your desktop\'s settings.'] },
  pair: { why: 'Your phone isn\'t paired with this computer over Bluetooth yet. Calls use Bluetooth, the way a car or headset does.',
    steps: ['Open Bluetooth settings on this computer and pair it with your phone.', 'When the phone asks, allow Phone calls (and Contacts if you like) for this computer.'] },
  connect: { why: 'Your phone is paired over Bluetooth but not connected for calls.',
    steps: ['On the phone, open Settings, Connections, Bluetooth, then the gear next to this computer, and turn on Calls.', 'Or connect it from Bluetooth settings on this computer.'] },
};

function liveCalls() { return calls.status?.calls || []; }
function btCallsLive() { return liveCalls().length > 0; }
function callTitle(c) { return c.name || (c.number ? prettyNumber(c.number) : 'Unknown caller'); }

function loadCalls() {
  api('call_status').then(onCalls).catch(() => {});
}

function onCalls(status) {
  const prev = calls.status;
  calls.status = status;
  // The pressed button waits until the phone acts: the call changes state or ends, or the sound moves.
  const b = calls.busy;
  if (b) {
    const was = prev?.calls?.find((c) => c.id === b.id), now = liveCalls().find((c) => c.id === b.id);
    if (b.id === null ? status.audio_here : (!now || !was || now.state !== was.state)) calls.busy = null;
  }
  renderCallBanner();
  if ($('.rail')) renderRail();
}

function callStateText(c) {
  return { incoming: 'Incoming call', waiting: 'Call waiting', dialing: 'Calling', alerting: 'Ringing', held: 'On hold' }[c.state] || '';
}

function callClock(since) {
  const s = Math.max(0, Math.floor((Date.now() - since) / 1000));
  const mm = String(Math.floor(s / 60) % 60).padStart(2, '0'), ss = String(s % 60).padStart(2, '0');
  return s >= 3600 ? `${Math.floor(s / 3600)}:${mm}:${ss}` : `${mm}:${ss}`;
}

async function callAction(c, method, label) {
  calls.busy = { id: c ? c.id : null, label };
  renderCallBanner();
  try {
    await api(method, c ? { id: c.id } : {});
  } catch (e) {
    calls.busy = null;
    renderCallBanner();
    toast(String(e), 'error');
  }
  // The phone may take a moment; never leave the buttons stuck if nothing comes back.
  setTimeout(() => { if (calls.busy?.label === label) { calls.busy = null; renderCallBanner(); } }, 8000);
}

/** Mutes or unmutes this computer's microphone for the call; the daemon's next status confirms it. */
async function setCallMute(muted) {
  if (calls.muting) return;
  calls.muting = true;
  renderCallBanner();
  try {
    await api('call_mute', { muted });
    if (calls.status) calls.status.muted = muted;
  } catch (e) {
    toast(String(e), 'error');
  } finally {
    calls.muting = false;
    renderCallBanner();
    // The button was disabled while waiting, which drops focus; give it back.
    $('#call-root .btn.mute')?.focus();
  }
}

function muteButton() {
  const muted = !!calls.status?.muted;
  return h('button', { class: `btn mute${muted ? ' on' : ''}`, 'data-key': 'mute', 'aria-pressed': String(muted), disabled: calls.muting,
    title: muted ? 'Turn this computer\'s microphone back on' : 'Turn off this computer\'s microphone',
    onclick: () => setCallMute(!muted) }, icon(muted ? 'mic-off' : 'mic', 'sm'), muted ? 'Unmute' : 'Mute');
}

const KEYPAD = [['1', ''], ['2', 'ABC'], ['3', 'DEF'], ['4', 'GHI'], ['5', 'JKL'], ['6', 'MNO'],
  ['7', 'PQRS'], ['8', 'TUV'], ['9', 'WXYZ'], ['*', ''], ['0', '+'], ['#', '']];

/** One key press into the call (menus, PINs). Presses go out in order, one after another. */
function pressKey(k) {
  calls.keyed = (calls.keyed + k).slice(-24);
  const shown = $('#call-root .keyed');
  if (shown) shown.textContent = calls.keyed;
  calls.tones = calls.tones.then(() => api('call_tones', { tones: k })).catch((e) => toast(String(e), 'error'));
}

function toggleKeypad(c) {
  calls.keypad = calls.keypad === c.id ? null : c.id;
  calls.keyed = '';
  renderCallBanner();
  $(calls.keypad ? '#call-root .keypad button' : '#call-root .btn.keys')?.focus();
}

function keypad() {
  return h('div', { class: 'keypad-wrap', onkeydown: (e) => {
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    if (/^[0-9*#]$/.test(e.key)) { e.preventDefault(); pressKey(e.key); } else if (e.key === 'Escape') { e.stopPropagation(); toggleKeypad({ id: calls.keypad }); }
  } },
  h('div', { class: 'keyed', 'aria-live': 'polite', 'aria-label': 'Keys sent', text: calls.keyed }),
  h('div', { class: 'keypad', role: 'group', 'aria-label': 'Keypad' },
    KEYPAD.map(([k, sub]) => h('button', { class: 'btn', 'data-key': `key-${k}`, 'aria-label': k, onclick: () => pressKey(k) },
      h('span', { class: 'digit', text: k }), h('span', { class: 'letters', 'aria-hidden': 'true', text: sub })))));
}

function callCard(c) {
  const busy = calls.busy && (calls.busy.id === c.id || calls.busy.id === null) ? calls.busy.label : null;
  const ringing = c.state === 'incoming' || c.state === 'waiting';
  const active = c.state === 'active';
  const sub = active && c.since ? h('span', { class: 'call-timer', 'data-since': String(c.since), text: callClock(c.since) }) : h('span', { text: callStateText(c) });
  const where = active ? h('span', { text: calls.status.audio_here ? 'Sound on this computer' : 'Sound on your phone' }) : null;
  const mutedNote = !ringing && calls.status.muted ? h('span', { class: 'muted-note', text: 'Microphone off' }) : null;
  const title = callTitle(c);
  const actions = busy ? [h('div', { class: 'spinner', 'aria-hidden': 'true' }), h('span', { text: busy })]
    : ringing ? [
      h('button', { class: 'btn answer', onclick: () => callAction(c, 'call_answer', 'Answering') }, icon('call', 'sm'), 'Answer'),
      h('button', { class: 'btn hangup', onclick: () => callAction(c, 'call_hangup', 'Declining') }, icon('hangup', 'sm'), 'Decline')]
    : [
      active && !calls.status.audio_here ? h('button', { class: 'btn', onclick: () => callAction(null, 'call_audio_here', 'Moving the sound here') }, 'Use this computer') : null,
      // Two calls at once: the held one can take over (the other goes on hold).
      c.state === 'held' ? h('button', { class: 'btn', onclick: () => callAction(c, 'call_swap', 'Switching') }, icon('call', 'sm'), 'Switch to this call') : null,
      calls.status.can_mute && c.state !== 'held' ? muteButton() : null,
      active ? h('button', { class: 'btn keys', 'data-key': 'keys', 'aria-expanded': String(calls.keypad === c.id), title: 'Press keys for phone menus',
        onclick: () => toggleKeypad(c) }, icon('dialpad', 'sm'), calls.keypad === c.id ? 'Hide keypad' : 'Keypad') : null,
      h('button', { class: 'btn hangup', onclick: () => callAction(c, 'call_hangup', 'Hanging up') }, icon('hangup', 'sm'), 'Hang up')];
  return h('section', { class: 'call-card', 'aria-label': `${callStateText(c) || 'Call'}: ${title}` },
    h('div', { class: 'who' }, avatar(title, false),
      h('div', { class: 'grow' }, h('b', { text: title }),
        h('div', { class: 'sub' }, sub, c.name && c.number ? h('span', { text: prettyNumber(c.number) }) : null, where, mutedNote))),
    active && calls.keypad === c.id && !busy ? keypad() : null,
    h('div', { class: 'acts' }, actions));
}

function renderCallBanner() {
  let root = $('#call-root');
  if (!root) {
    root = h('div', { id: 'call-root', class: 'call-root', role: 'region', 'aria-label': 'Phone calls', 'aria-live': 'polite' });
    document.body.append(root);
  }
  // The keypad belongs to its call: it closes when that call ends or is no longer active.
  if (calls.keypad && !liveCalls().some((c) => c.id === calls.keypad && c.state === 'active')) { calls.keypad = null; calls.keyed = ''; }
  swap(root, () => liveCalls().map(callCard));
  const ticking = liveCalls().some((c) => c.state === 'active' && c.since);
  if (ticking && !calls.timer) {
    calls.timer = setInterval(() => {
      for (const el of document.querySelectorAll('.call-timer')) el.textContent = callClock(Number(el.dataset.since));
    }, 1000);
  } else if (!ticking && calls.timer) {
    clearInterval(calls.timer);
    calls.timer = null;
  }
}

function closeDialog() { $('#palette-root').replaceChildren(); }

function dialog(label, ...children) {
  const root = $('#palette-root');
  root.replaceChildren(h('div', { class: 'palette-backdrop', onclick: (e) => { if (e.target === e.currentTarget) closeDialog(); },
    onkeydown: (e) => { if (e.key === 'Escape') closeDialog(); } },
  h('div', { class: 'palette call-dialog', role: 'dialog', 'aria-modal': 'true', 'aria-label': label }, ...children)));
  return root;
}

/** Explains what's missing for calls, with a way to check again once it's done. */
function callSetupDialog(then) {
  const s = calls.status || { blocker: 'service' };
  const info = callSetup[s.blocker] || callSetup.service;
  const again = h('button', { class: 'btn primary', onclick: async () => {
    again.disabled = true;
    again.replaceChildren(h('div', { class: 'spinner', 'aria-hidden': 'true' }), 'Checking');
    try { onCalls(await api('call_status')); } catch { /* daemon restarting */ }
    if (!calls.status?.blocker) { closeDialog(); if (then) then(); } else callSetupDialog(then);
  } }, 'Check again');
  dialog('Calls from this computer',
    h('h2', { text: 'Calls from this computer' }),
    h('p', { text: info.why }),
    h('ol', { class: 'steps' }, info.steps.map((t) => h('li', { text: t }))),
    s.phone && s.blocker === 'connect' ? h('p', { class: 'muted', text: `Your phone shows up in Bluetooth as ${s.phone}.` }) : null,
    h('div', { class: 'row' }, again, h('button', { class: 'btn ghost', onclick: closeDialog }, 'Close')));
  again.focus();
}

async function dialNumber(number, name) {
  if (!calls.status) { try { calls.status = await api('call_status'); } catch { /* shown below */ } }
  if (calls.status?.blocker) return callSetupDialog(() => dialNumber(number, name));
  // Until the phone takes the number; then the call banner shows the call's progress.
  const t = toast(`Calling ${name || prettyNumber(number)}`, 'progress', 8000);
  try { await api('call_dial', { number }); } catch (e) { toast(String(e), 'error'); } finally { t.remove(); }
}

function dialDialog() {
  if (calls.status?.blocker) return callSetupDialog(dialDialog);
  const input = h('input', { type: 'tel', placeholder: 'Phone number', 'aria-label': 'Phone number', autocomplete: 'off' });
  const go = h('button', { class: 'btn primary', type: 'submit' }, icon('call', 'sm'), 'Call');
  dialog('Call a number',
    h('form', { onsubmit: (e) => { e.preventDefault(); const n = input.value.trim(); if (!n) return input.focus(); closeDialog(); dialNumber(n); } },
      h('h2', { text: 'Call a number' }), input,
      h('div', { class: 'row' }, go, h('button', { class: 'btn ghost', type: 'button', onclick: closeDialog }, 'Cancel'))));
  input.focus();
}

/** A Call button for a one-person conversation or contact. */
function callButton(number, name) {
  if (!number || !digits(number)) return null;
  return h('button', { class: 'btn ghost icon-only', title: `Call ${name || prettyNumber(number)}`, 'aria-label': `Call ${name || prettyNumber(number)}`, onclick: () => dialNumber(number, name) }, icon('call'));
}

function activeCallCommands() {
  return liveCalls().flatMap((c) => {
    const who = callTitle(c);
    if (c.state === 'incoming' || c.state === 'waiting') {
      return [{ label: `Answer ${who}`, icon: 'call', run: () => callAction(c, 'call_answer', 'Answering') },
        { label: `Decline ${who}`, icon: 'hangup', run: () => callAction(c, 'call_hangup', 'Declining') }];
    }
    return [{ label: `Hang up on ${who}`, icon: 'hangup', run: () => callAction(c, 'call_hangup', 'Hanging up') },
      c.state === 'active' && calls.keypad !== c.id ? { label: `Keypad for ${who}`, icon: 'dialpad', run: () => toggleKeypad(c) } : null,
      c.state === 'held' ? { label: `Switch to ${who}`, sub: 'The other call goes on hold', icon: 'call', run: () => callAction(c, 'call_swap', 'Switching') } : null].filter(Boolean);
  }).concat(liveCalls().some((c) => c.state !== 'incoming' && c.state !== 'waiting') && calls.status?.can_mute
    ? [calls.status.muted ? { label: 'Unmute the microphone', icon: 'mic', run: () => setCallMute(false) }
      : { label: 'Mute the microphone', icon: 'mic-off', run: () => setCallMute(true) }]
    : []);
}

function dialCommands() {
  return [{ label: 'Call a number', icon: 'call', run: dialDialog },
    ...state.contacts.map((c) => ({ label: `Call ${c.name}`, sub: prettyNumber(c.number), icon: 'call', run: () => dialNumber(c.number, c.name) }))];
}

// ---------- texts sent later ----------
// The service holds the schedule, so texts go out with the window closed; this is only the view.

function loadScheduled() {
  api('scheduled_list').then((items) => { scheduled.items = items; invalidate('thread'); }).catch(() => {});
  // The palette's quiet mode entry reads the settings.
  if (!settings.data) loadSettings();
}

function laterButton(ta, d) {
  return h('button', { class: 'btn ghost icon-only', 'aria-label': 'Send later', title: 'Send later', 'data-key': 'send-later', disabled: !(d && d.connected),
    onclick: () => {
      if (!ta.value.trim()) { ta.focus(); return toast('Write the message first', 'error'); }
      openSchedule({ body: ta.value.trim(), composer: ta });
    } }, icon('clock'));
}

/** "Today 6:00 PM", "Tomorrow 9:00 AM", "Fri, Nov 21, 9:00 AM". */
function whenLabel(ms) {
  const d = new Date(ms);
  const time = d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
  const today = new Date();
  const tomorrow = new Date(today); tomorrow.setDate(today.getDate() + 1);
  if (sameDay(d, today)) return `today at ${time}`;
  if (sameDay(d, tomorrow)) return `tomorrow at ${time}`;
  return `${d.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' })} at ${time}`;
}

function laterChoices(now = new Date()) {
  const at = (days, hour) => { const d = new Date(now); d.setDate(d.getDate() + days); d.setHours(hour, 0, 0, 0); return d.getTime(); };
  const list = [{ label: 'In 1 hour', at: now.getTime() + 3600e3 }];
  if (now.getHours() < 17) list.push({ label: 'This evening', at: at(0, 18) });
  list.push({ label: 'Tomorrow morning', at: at(1, 9) });
  return list;
}

// The value a datetime-local field takes: local time, no seconds.
function localInputValue(ms) {
  const d = new Date(ms - new Date(ms).getTimezoneOffset() * 60e3);
  return d.toISOString().slice(0, 16);
}

/** The schedule dialog: a new text from the composer, or `item` to change. */
function openSchedule({ body = '', composer = null, item = null }) {
  const root = $('#palette-root');
  const close = () => { root.replaceChildren(); (composer || $('.composer textarea'))?.focus(); };
  let target = null;
  if (!item) {
    if (state.selected === 'new') {
      if (!state.newTo.length) return toast('Add who the message is for first', 'error');
      target = { addresses: [...state.newTo], thread_id: null };
    } else {
      target = { addresses: (threadMessages(state.selected)[0]?.addresses || []).map((a) => a.address), thread_id: state.selected };
    }
  }
  const text = h('textarea', { 'aria-label': 'Message', rows: 3, required: true });
  text.value = item ? item.body : body;
  const error = h('p', { class: 'form-error', role: 'alert', hidden: true });
  const custom = h('input', { type: 'datetime-local', id: 'later-at', required: true, min: localInputValue(Date.now()),
    value: localInputValue(item ? item.at : Date.now() + 3600e3) });
  const buttons = [];
  const pick = async (at) => {
    const value = text.value.trim();
    if (!value) { error.textContent = 'Write the message first.'; error.hidden = false; return text.focus(); }
    if (!Number.isFinite(at)) { error.textContent = 'Pick a date and time.'; error.hidden = false; return custom.focus(); }
    buttons.forEach((b) => { b.disabled = true; });
    error.hidden = true;
    try {
      const saved = item
        ? await api('schedule_edit', { id: item.id, body: value, at })
        : await api('schedule_add', { device: state.deviceId, ...target, body: value, at });
      scheduled.items = [...scheduled.items.filter((s) => s.id !== saved.id), { ...saved, waiting: false }];
      if (composer) { composer.value = ''; composer.style.height = 'auto'; }
      root.replaceChildren();
      invalidate('thread');
      toast(`It will send ${whenLabel(saved.at)}`);
    } catch (e) {
      error.textContent = e.message || String(e); error.hidden = false;
      buttons.forEach((b) => { b.disabled = false; });
    }
  };
  for (const c of laterChoices()) {
    buttons.push(h('button', { class: 'later-choice', type: 'button', onclick: () => pick(c.at) },
      h('b', { text: c.label }), h('span', { text: whenLabel(c.at).replace(/^./, (x) => x.toUpperCase()) })));
  }
  const submit = h('button', { class: 'btn primary', type: 'submit' }, item ? 'Save' : 'Schedule');
  const cancel = h('button', { class: 'btn ghost', type: 'button', onclick: close }, 'Cancel');
  buttons.push(submit);
  root.replaceChildren(h('div', { class: 'palette-backdrop', onclick: (e) => { if (e.target === e.currentTarget) close(); } },
    h('form', { class: 'palette schedule-dialog', role: 'dialog', 'aria-label': item ? 'Change scheduled text' : 'Send later',
      onkeydown: (e) => { if (e.key === 'Escape') { e.stopPropagation(); close(); } },
      onsubmit: (e) => { e.preventDefault(); pick(new Date(custom.value).getTime()); } },
    h('h2', { text: item ? 'Change scheduled text' : 'Send later' }),
    text,
    h('div', { class: 'later-choices' }, buttons.filter((b) => b !== submit)),
    h('label', { for: 'later-at', class: 'later-custom' }, h('small', { text: 'Or pick a date and time' })),
    h('div', { class: 'field-row' }, custom, submit),
    error,
    h('p', { class: 'hint', text: 'Tether sends it from your phone, even with this window closed. If your phone is out of reach then, it goes out once it is back.' }),
    h('div', { class: 'dialog-actions' }, cancel))));
  (buttons[0] || text).focus();
}

function scheduledNodes(id, msgs) {
  const addrs = msgs[0]?.addresses || [];
  const items = scheduled.items
    .filter((s) => s.device === state.deviceId && (s.thread_id === id || (s.thread_id == null && addrs.length && sameAddrs(s.addresses, addrs))))
    .sort((a, b) => a.at - b.at);
  if (!items.length) return [];
  return [h('div', { class: 'day', text: items.length === 1 ? 'Scheduled' : `${items.length} scheduled` }), ...items.map((s) => {
    const status = s.state === 'unsure' ? 'Tether stopped while sending this, so it may have gone out. Check on your phone.'
      : s.state === 'sending' ? 'Sending'
        : s.waiting ? 'Waiting for your phone. It sends once your phone is back.'
          : `Sends ${whenLabel(s.at)}`;
    const busy = s.state === 'sending';
    return h('div', { class: `bubble-row out first scheduled ${s.waiting || s.state === 'unsure' ? 'waiting' : ''}`, 'data-key': `sched-${s.id}` },
      h('div', { class: 'bubble', text: s.body }),
      h('div', { class: 'stamp' }, h('span', { class: 'stamp-status' }, busy ? h('span', { class: 'spinner' }) : icon('clock', 'sm'), h('span', { text: status })),
        busy ? null : h('span', { class: 'stamp-actions' },
          h('button', { class: 'stamp-btn', onclick: () => openSchedule({ item: s }), 'aria-label': 'Change this scheduled text' }, s.state === 'unsure' ? 'Send again' : 'Edit'),
          h('button', { class: 'stamp-btn', onclick: () => cancelScheduled(s), 'aria-label': 'Cancel this scheduled text' }, 'Cancel'))));
  })];
}

async function cancelScheduled(s) {
  try {
    await api('schedule_cancel', { id: s.id });
    scheduled.items = scheduled.items.filter((x) => x.id !== s.id);
    invalidate('thread');
    toast('Scheduled text cancelled');
    $('.composer textarea')?.focus();
  } catch (e) { toast(e.message || String(e), 'error'); }
}

// ---------- quiet mode ----------
function quietSettings() {
  const s = settings.data;
  return [
    settingSwitch('quiet_mode', 'Quiet mode', 'Phone notifications come as one summary now and then instead of one pop-up each. Calls and texts with sign-in codes still pop up right away.'),
    s.quiet_mode ? h('div', { class: 'setting row-setting' }, h('label', { for: 'quiet-interval' }, h('b', { text: 'Summary' }), h('small', { text: 'Only when something came in.' })),
      h('select', { id: 'quiet-interval', 'data-key': 'set-quiet_interval', onchange: (e) => saveSettings({ quiet_interval: Number(e.target.value) }) },
        [[30, 'At most every 30 minutes'], [60, 'At most every hour'], [120, 'At most every 2 hours'], [240, 'At most every 4 hours']]
          .map(([value, label]) => h('option', { value, selected: s.quiet_interval === value }, label)))) : null,
  ];
}

function quietCommand() {
  const s = settings.data;
  if (!s) return null;
  const on = !!s.quiet_mode;
  return { label: on ? 'Turn off quiet mode' : 'Turn on quiet mode', sub: 'Notification summary', icon: 'bell',
    run: () => saveSettings({ quiet_mode: !on }, on ? 'Quiet mode is off' : 'Quiet mode is on') };
}

// A click on the quiet mode summary pop-up.
function showNotifications() {
  if (state.view !== 'messages') showView('messages');
  if (!state.feedOpen) toggleFeed();
}
