'use strict';

// The tray flyout: battery, mic and the active custom preset. It reads the
// headset each time it opens and follows every change the app makes.

const tauri = window.__TAURI__;
const $ = id => document.getElementById(id);
const PRESETS = [1, 2, 3, 4];
let state = null;
let busy = false;
let failure = '';

function headline(s) {
  if (s.offline) return 'Offline: headset controls are off';
  if (s.audeze_running) return 'Close Audeze HQ to use the headset';
  if (s.battery != null) return 'Battery';
  if (s.error) return 'Headset not found';
  return 'Reading the headset…';
}

function usable(s) {
  return s && !s.offline && !s.audeze_running;
}

function eqLabel(s) {
  if (s.slot) return `EQ ${s.slot}`;
  return s.preset_name || 'EQ';
}

// Ctrl+Alt+1 as Windows writes it; the Win key is stored as Super.
function keysText(keys) {
  return (keys || '').replace(/\bSuper\b/g, 'Win');
}

function render(next) {
  if (!next) return;
  state = next;
  const root = document.documentElement;
  root.dataset.theme = state.light ? 'light' : 'dark';
  if (state.accent) root.style.setProperty('--accent', state.accent);
  else root.style.removeProperty('--accent');

  const battery = state.battery;
  const known = battery != null && usable(state);
  $('level').textContent = known ? `${battery}%` : '—';
  $('caption').textContent = headline(state);
  const fill = $('arc-fill');
  const level = known ? Math.max(0, Math.min(100, battery)) : 0;
  fill.style.strokeDasharray = `${level} 100`;
  fill.classList.toggle('empty', level === 0);
  document.querySelector('.gauge').classList.toggle('low', known && battery <= 15);
  $('level').setAttribute('aria-label', known ? `Battery ${battery} percent` : headline(state));

  const mic = $('mic');
  const muted = known ? state.mic_muted : null;
  const micText = muted === true ? 'Mic muted' : muted === false ? 'Mic on' : 'Mic state unknown';
  mic.classList.toggle('muted', muted === true);
  mic.classList.toggle('unknown', muted == null);
  mic.title = micText;
  mic.setAttribute('aria-label', micText);
  $('mic-icon').setAttribute('href', muted === true ? '#i-mic-off' : '#i-mic');

  const button = $('eq-button');
  $('eq-label').textContent = eqLabel(state);
  button.disabled = !usable(state);
  button.setAttribute('aria-busy', String(busy));
  const profile = state.slot && state.profile ? `${eqLabel(state)}: ${state.profile}` : eqLabel(state);
  button.title = `${profile}. Choose a custom preset.`;
  if (!$('eq-menu').hidden) fillMenu();

  const device = $('device');
  const parts = typeof state.device === 'string' ? state.device.split(' · ').slice(0, 2) : [];
  device.textContent = failure || (parts.length ? parts.join(' · ') : 'Maxwell PEQ');
  device.classList.toggle('error', Boolean(failure));
}

function fillMenu() {
  const menu = $('eq-menu');
  menu.replaceChildren(...PRESETS.map(slot => {
    const item = document.createElement('button');
    item.type = 'button';
    item.setAttribute('role', 'menuitemradio');
    item.setAttribute('aria-checked', String(state?.slot === slot));
    item.dataset.slot = slot;
    const check = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    check.classList.add('check');
    const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
    use.setAttribute('href', '#i-check');
    check.append(use);
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = `EQ ${slot}`;
    const profile = state?.profiles?.[slot - 1];
    if (profile) {
      const small = document.createElement('small');
      small.textContent = profile;
      name.append(small);
    }
    const keys = document.createElement('kbd');
    keys.textContent = keysText(state?.shortcuts?.[`preset${slot}`]);
    item.append(check, name, keys);
    item.addEventListener('click', () => choose(slot));
    return item;
  }));
}

function openMenu() {
  fillMenu();
  $('eq-menu').hidden = false;
  $('eq-button').setAttribute('aria-expanded', 'true');
  const items = [...$('eq-menu').querySelectorAll('button')];
  (items.find(item => item.getAttribute('aria-checked') === 'true') || items[0])?.focus();
}

function closeMenu(focusButton = false) {
  if ($('eq-menu').hidden) return;
  $('eq-menu').hidden = true;
  $('eq-button').setAttribute('aria-expanded', 'false');
  if (focusButton) $('eq-button').focus();
}

async function choose(slot) {
  closeMenu(true);
  if (busy) return;
  busy = true;
  failure = '';
  render(state);
  try {
    const next = await tauri.core.invoke('tray_preset', { value: slot });
    busy = false;
    render(next);
  } catch (error) {
    busy = false;
    failure = String(error);
    render(state);
  }
}

async function refresh() {
  try {
    render(await tauri.core.invoke('tray_status', { refresh: true }));
  } catch (error) {
    failure = String(error);
    render(state);
  }
}

$('eq-button').addEventListener('click', () => ($('eq-menu').hidden ? openMenu() : closeMenu(true)));
$('open-editor').addEventListener('click', () => tauri.core.invoke('flyout_open_editor'));
document.addEventListener('pointerdown', event => {
  if (!event.target.closest('#eq-menu, #eq-button')) closeMenu();
});
document.addEventListener('keydown', event => {
  if (event.key === 'Escape') {
    if (!$('eq-menu').hidden) closeMenu(true);
    else tauri.core.invoke('flyout_hide');
    return;
  }
  if ($('eq-menu').hidden) return;
  const items = [...$('eq-menu').querySelectorAll('button')];
  const at = items.indexOf(document.activeElement);
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    items[(at + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length].focus();
  } else if (event.key === 'Home' || event.key === 'End') {
    event.preventDefault();
    items[event.key === 'Home' ? 0 : items.length - 1].focus();
  } else if (/^[1-4]$/.test(event.key)) {
    choose(Number(event.key));
  }
});
document.addEventListener('contextmenu', event => event.preventDefault());

tauri.event.listen('status-changed', event => render(event.payload));
tauri.event.listen('command-failed', event => { failure = String(event.payload); render(state); });
tauri.event.listen('flyout-shown', event => {
  failure = '';
  closeMenu();
  render(event.payload);
  refresh();
});
tauri.core.invoke('tray_status', { refresh: false }).then(render, () => {});
