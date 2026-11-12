// Inline SVG icons (stroke style, 24px grid). Never font glyphs: they render as boxes on some systems.
const ICONS = {
  apps: '<rect x="4" y="4" width="6.5" height="6.5" rx="1.5"/><rect x="13.5" y="4" width="6.5" height="6.5" rx="1.5"/><rect x="4" y="13.5" width="6.5" height="6.5" rx="1.5"/><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.5"/>',
  pin: '<path d="M9 4h6l-1 6 3 3v2H7v-2l3-3-1-6Z"/><path d="M12 15v5"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
  send: '<path d="M4 12 20 4l-6 16-3-7-7-1Z"/>',
  bell: '<path d="M6 16V11a6 6 0 1 1 12 0v5l2 2H4l2-2Z"/><path d="M10 20a2 2 0 0 0 4 0"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h8"/>',
  phone: '<rect x="7" y="2.5" width="10" height="19" rx="2.5"/><path d="M11 18.5h2"/>',
  ring: '<path d="M6 16V11a6 6 0 1 1 12 0v5l2 2H4l2-2Z"/><path d="M2.5 9a9 9 0 0 1 2.5-5M21.5 9A9 9 0 0 0 19 4"/>',
  message: '<path d="M4 5h16v11H9l-5 4V5Z"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  upload: '<path d="M12 16V4M7 9l5-5 5 5"/><path d="M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3"/>',
  refresh: '<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 5v6h-6"/>',
  reply: '<path d="M10 8 5 12l5 4"/><path d="M5 12h9a5 5 0 0 1 5 5v1"/>',
  bolt: '<path d="M13 3 5 13h6l-1 8 8-10h-6l1-8Z"/>',
  key: '<circle cx="8" cy="15" r="4"/><path d="m11 12 9-9M17 6l3 3M15 8l2 2"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
  mute: '<path d="M6 16V11a6 6 0 0 1 9.5-4.9M18 11v5l2 2H8"/><path d="M10 20a2 2 0 0 0 4 0M3 3l18 18"/>',
  wifi: '<path d="M2 9a15 15 0 0 1 20 0M5 12.5a10 10 0 0 1 14 0M8.5 16a5 5 0 0 1 7 0"/><circle cx="12" cy="19.5" r="1"/>',
  command: '<path d="M9 6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6Z"/>',
  alert: '<path d="M12 3 2 20h20L12 3Z"/><path d="M12 10v4M12 17h.01"/>',
  chevron: '<path d="m9 6 6 6-6 6"/>',
  play: '<path d="M8 5.5v13l10.5-6.5L8 5.5Z" fill="currentColor"/>',
  pause: '<path d="M8.5 5.5v13M15.5 5.5v13" stroke-width="3"/>',
  next: '<path d="M6 6.5v11l8-5.5-8-5.5Z" fill="currentColor"/><path d="M18 6v12"/>',
  prev: '<path d="M18 6.5v11l-8-5.5 8-5.5Z" fill="currentColor"/><path d="M6 6v12"/>',
  image: '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="m4 18 5-5 4 4 2.5-2.5L20 18"/>',
  download: '<path d="M12 4v12M7 11l5 5 5-5M5 20h14"/>',
  music: '<path d="M9 18V6l11-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="16" r="2.5"/>',
  call: '<path d="M5 4h4l2 5-2.5 1.5a11 11 0 0 0 5 5L15 13l5 2v4a1 1 0 0 1-1 1A16 16 0 0 1 4 5a1 1 0 0 1 1-1Z"/>',
};

function icon(name, cls = '') {
  const span = document.createElement('span');
  span.className = 'icon ' + cls;
  span.setAttribute('aria-hidden', 'true');
  span.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] || ''}</svg>`;
  return span;
}

/** Battery drawn to scale so the level reads at a glance, not just as a number. */
function batteryIcon(level, charging) {
  const span = document.createElement('span');
  span.className = 'icon battery';
  span.setAttribute('aria-hidden', 'true');
  const w = Math.max(0, Math.min(100, level)) / 100 * 13;
  const low = level >= 0 && level <= 15 && !charging;
  span.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round">
    <rect x="2.5" y="7" width="17" height="10" rx="2.5"/><path d="M21.5 10.5v3" stroke-linecap="round"/>
    <rect x="4.5" y="9" width="${w.toFixed(1)}" height="6" rx="1" fill="currentColor" stroke="none" class="${low ? 'low' : ''}"/>
    ${charging ? '<path d="M11.5 8.5 9 12.3h3l-1.5 3.2" stroke="var(--bg-raised)" stroke-width="1.6" stroke-linecap="round"/>' : ''}
  </svg>`;
  return span;
}

function signalIcon(bars) {
  const span = document.createElement('span');
  span.className = 'icon signal';
  span.setAttribute('aria-hidden', 'true');
  let rects = '';
  for (let i = 0; i < 4; i++) {
    const h = 4 + i * 4;
    rects += `<rect x="${3 + i * 5}" y="${20 - h}" width="3" height="${h}" rx="1" fill="currentColor" opacity="${i < bars ? 1 : 0.25}"/>`;
  }
  span.innerHTML = `<svg viewBox="0 0 24 24">${rects}</svg>`;
  return span;
}

document.querySelectorAll('[data-icon]').forEach((el) => el.replaceWith(icon(el.dataset.icon, el.className)));
