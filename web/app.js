'use strict';
const $ = id => document.getElementById(id);
const {SHELF_Q, isPass, isShelf} = PEQGraph;
const TYPES = {peak: 'Peak', low_shelf: 'Low shelf', high_shelf: 'High shelf', low_pass: 'Low-pass', high_pass: 'High-pass'};
const MAX_FILTERS = 10;

const state = {
  token: null, status: null, transportDirty: false, busy: null,
  library: {profiles: [], active: null}, preset: null, selected: 0,
  history: new Map(), committed: null,
  compiled: null, compileError: null, attempted: new Set(),
  audezeDismissed: false, recoveryShown: false, profileKey: '',
};

/* ---------- formatting ---------- */
const round = (value, digits) => Math.round(value * 10 ** digits) / 10 ** digits;
const minus = text => text.replace(/^-/, '−');
const fmtDb = (value, digits = 1) => `${value > 0 ? '+' : ''}${minus(value.toFixed(digits))} dB`;
const fmtHz = f => f >= 1000 ? `${round(f / 1000, f >= 10000 ? 1 : 2)} kHz` : `${round(f, 1)} Hz`;
const clone = value => JSON.parse(JSON.stringify(value));

/* ---------- requests ---------- */
// In the desktop app requests go over Tauri IPC; in a browser (`--serve`) over HTTP with the session token.
const tauri = window.__TAURI__;

async function api(path, body, options = {}) {
  const post = body !== undefined;
  if (tauri) {
    try { return await (post ? tauri.core.invoke('post', {path, body}) : tauri.core.invoke('get', {path})); }
    catch (error) { throw new Error(typeof error === 'string' ? error : error?.message || String(error)); }
  }
  const response = await fetch(path, {
    method: post ? 'POST' : 'GET', keepalive: !!options.keepalive,
    headers: post ? {'Content-Type': 'application/json', 'X-Session-Token': state.token || ''} : {},
    ...(post ? {body: JSON.stringify(body)} : {}),
  });
  let result;
  try { result = await response.json(); } catch { throw new Error(`The app's local server sent an unreadable reply (${response.status}).`); }
  if (!response.ok || result.error) throw new Error(result.error || `Request failed (${response.status}).`);
  return result;
}

// Compiles and device operations share the server's device lock, so they run one at a time.
let deviceChain = Promise.resolve();
const queue = task => { const run = deviceChain.then(task, task); deviceChain = run.catch(() => {}); return run; };
let saveChain = Promise.resolve();
const queueSave = task => { const run = saveChain.then(task, task); saveChain = run.catch(() => {}); return run; };

/* ---------- toasts ---------- */
function toast(text, kind = 'info') {
  const box = $('toasts');
  const item = document.createElement('div');
  item.className = `toast ${kind}`;
  if (kind === 'error') item.setAttribute('role', 'alert');
  const message = document.createElement('p'); message.textContent = text;
  const close = document.createElement('button');
  close.type = 'button'; close.className = 'icon-button'; close.setAttribute('aria-label', 'Dismiss');
  close.innerHTML = '<svg><use href="#i-close"/></svg>';
  const remove = () => { item.classList.add('leaving'); setTimeout(() => item.remove(), 160); };
  close.addEventListener('click', remove);
  item.append(message, close);
  box.append(item);
  while (box.children.length > 3) box.firstElementChild.remove();
  setTimeout(remove, kind === 'error' ? 9000 : 5000);
}

/* ---------- presets ---------- */
const flatPreset = () => ({filters: [{type: 'peak', frequency: 1000, gain: 0, q: 1, enabled: true}], preamp: 'auto'});

function normalizeFilter(f) {
  const type = TYPES[f.type] ? f.type : 'peak';
  return {type, frequency: f.frequency ?? null, gain: isPass(type) ? 0 : f.gain ?? null,
    q: isShelf(type) ? SHELF_Q : f.q ?? null, enabled: f.enabled !== false};
}
const payload = (preset = state.preset) => ({filters: preset.filters.map(normalizeFilter), preamp: preset.preamp ?? 'auto'});
const r6 = v => typeof v === 'number' ? round(v, 6) : v;
function canon(preset) {
  if (!preset || !Array.isArray(preset.filters)) return null;
  const p = payload(preset);
  return JSON.stringify({filters: p.filters.map(f => ({type: f.type, frequency: r6(f.frequency), gain: r6(f.gain), q: r6(f.q), enabled: f.enabled})),
    preamp: p.preamp === 'auto' ? 'auto' : r6(p.preamp)});
}
const signature = () => canon(state.preset);

function problems(preset = state.preset) {
  const list = [];
  const inRange = (v, lo, hi) => typeof v === 'number' && Number.isFinite(v) && v >= lo && v <= hi;
  preset.filters.forEach((f, i) => {
    if (!inRange(f.frequency, 20, 20000)) list.push({index: i, field: 'frequency', text: `Filter ${i + 1}: frequency must be 20 to 20,000 Hz.`});
    if (!isPass(f.type) && !inRange(f.gain, -12, 12)) list.push({index: i, field: 'gain', text: `Filter ${i + 1}: gain must be −12 to +12 dB.`});
    if (!isShelf(f.type) && !inRange(f.q, 0.1, 20)) list.push({index: i, field: 'q', text: `Filter ${i + 1}: Q must be 0.1 to 20.`});
  });
  if (preset.preamp !== 'auto' && !inRange(preset.preamp, -60, 0)) list.push({index: -1, field: 'preamp', text: 'Manual preamp must be −60 to 0 dB.'});
  return list;
}

/* ---------- profiles ---------- */
const activeProfile = () => state.library.profiles.find(p => p.id === state.library.active);

function uniqueName(base) {
  const names = new Set(state.library.profiles.map(p => p.name.toLowerCase()));
  if (!names.has(base.toLowerCase())) return base;
  for (let n = 2; ; n++) if (!names.has(`${base} ${n}`.toLowerCase())) return `${base} ${n}`;
}

function historyFor(id) {
  if (!state.history.has(id)) state.history.set(id, {undo: [], redo: []});
  return state.history.get(id);
}

function loadActive() {
  const profile = activeProfile();
  state.preset = clone(profile.preset);
  state.preset.filters = state.preset.filters.map(normalizeFilter);
  state.committed = JSON.stringify(state.preset);
  state.selected = Math.min(state.selected, state.preset.filters.length - 1);
  if (state.selected < 0) state.selected = 0;
  $('profile-name').value = profile.name;
  renderAll();
  compileSoon(0);
}

let saveTimer = null, pendingSave = null;
function scheduleSave() {
  clearTimeout(saveTimer);
  const id = state.library.active, preset = payload();
  const profile = activeProfile(); if (profile) profile.preset = preset;
  pendingSave = {id, preset};
  saveTimer = setTimeout(() => flushSave(), 500);
}
function flushSave(keepalive = false) {
  clearTimeout(saveTimer);
  if (!pendingSave) return saveChain;
  const {id, preset} = pendingSave; pendingSave = null;
  return persist(id, {preset}, keepalive);
}
function persist(id, changes, keepalive = false) {
  return queueSave(() => api('/api/profiles/save', {id, ...changes}, {keepalive}))
    .catch(error => toast(`Couldn't save the profile: ${error.message}`, 'error'));
}

function adoptLibrary(result) {
  state.library = {profiles: result.profiles, active: result.active};
}

async function activate(id) {
  if (id === state.library.active) return;
  if (document.activeElement === $('profile-name')) $('profile-name').blur();
  await flushSave();
  const previous = state.library.active;
  state.library.active = id;
  const profile = activeProfile();
  state.attempted.delete(canon(profile.preset));
  state.selected = 0;
  loadActive();
  api('/api/profiles/activate', {id}).catch(error => {
    toast(error.message, 'error');
    if (!state.library.profiles.some(p => p.id === id)) { state.library.active = previous; loadActive(); }
  });
}

async function createProfile(name, preset) {
  await flushSave();
  const result = await api('/api/profiles/save', {name: uniqueName(name), preset, activate: true});
  adoptLibrary(result);
  state.selected = 0;
  loadActive();
  return result.saved;
}

/* ---------- editing ---------- */
function changed({compile = true, rows = false} = {}) {
  if (rows) renderRows(); else renderRowState();
  renderGraph(); renderEditorState(); renderDevice(); renderProfiles();
  scheduleSave();
  if (compile) compileSoon();
}

function commit() {
  const now = JSON.stringify(state.preset);
  if (now === state.committed) return;
  const history = historyFor(state.library.active);
  history.undo.push(state.committed);
  if (history.undo.length > 100) history.undo.shift();
  history.redo = [];
  state.committed = now;
  renderEditorState();
}

function step(direction) {
  commit();
  const history = historyFor(state.library.active);
  const from = direction < 0 ? history.undo : history.redo, to = direction < 0 ? history.redo : history.undo;
  if (!from.length) return;
  to.push(state.committed);
  state.committed = from.pop();
  state.preset = JSON.parse(state.committed);
  state.selected = Math.min(state.selected, state.preset.filters.length - 1);
  changed({rows: true});
}

function setFilter(index, values) {
  Object.assign(state.preset.filters[index], values);
}

function freeFrequency() {
  const used = state.preset.filters.map(f => Math.log10(Math.min(20000, Math.max(20, f.frequency || 1000)))).sort((a, b) => a - b);
  const points = [Math.log10(20), ...used, Math.log10(20000)];
  let best = [0, Math.log10(1000)];
  for (let i = 1; i < points.length; i++) {
    const gap = points[i] - points[i - 1];
    if (gap > best[0]) best = [gap, (points[i] + points[i - 1]) / 2];
  }
  const f = 10 ** best[1];
  return f < 1000 ? Math.round(f) : Math.round(f / 10) * 10;
}

function addFilter(frequency = freeFrequency(), gain = 0) {
  if (state.preset.filters.length >= MAX_FILTERS) { toast('The Maxwell holds 10 filters. Remove one first.', 'warn'); return; }
  commit();
  state.preset.filters.push({type: 'peak', frequency, gain, q: 1, enabled: true});
  state.selected = state.preset.filters.length - 1;
  changed({rows: true}); commit();
}

function removeFilter(index) {
  if (state.preset.filters.length <= 1) return;
  commit();
  state.preset.filters.splice(index, 1);
  state.selected = Math.min(state.selected, state.preset.filters.length - 1);
  changed({rows: true}); commit();
}

function select(index, {scroll = false} = {}) {
  if (index === state.selected) return;
  state.selected = index;
  renderRowState(); renderGraph();
  if (scroll) $('filter-rows').children[index]?.scrollIntoView({block: 'nearest'});
}

/* ---------- filter table ---------- */
function numberCell(index, field, unit, {min, max, stepValue}) {
  const cell = document.createElement('td');
  const wrap = document.createElement('span'); wrap.className = 'unit-input';
  const input = document.createElement('input');
  Object.assign(input, {type: 'number', min, max, step: 'any', inputMode: 'decimal'});
  input.dataset.field = field; input.dataset.step = stepValue;
  input.setAttribute('aria-label', `${{frequency: 'Frequency', gain: 'Gain', q: 'Q'}[field]}, filter ${index + 1}`);
  input.addEventListener('input', () => {
    setFilter(index, {[field]: input.value === '' ? null : Number(input.value)});
    changed();
  });
  input.addEventListener('change', commit);
  wrap.append(input);
  if (unit) { const label = document.createElement('span'); label.textContent = unit; wrap.append(label); }
  cell.append(wrap);
  return cell;
}

function fixedCell(text, title) {
  const cell = document.createElement('td');
  const value = document.createElement('span'); value.className = 'fixed-value'; value.textContent = text; value.title = title;
  cell.append(value);
  return cell;
}

function renderRows() {
  const body = $('filter-rows');
  body.replaceChildren();
  state.preset.filters.forEach((f, index) => {
    const row = document.createElement('tr');
    row.dataset.index = index;
    row.addEventListener('focusin', () => select(index));
    row.addEventListener('pointerdown', () => select(index));

    const on = document.createElement('td'); on.className = 'col-on';
    const toggle = document.createElement('input'); toggle.type = 'checkbox'; toggle.className = 'row-toggle';
    toggle.checked = f.enabled; toggle.setAttribute('aria-label', `Filter ${index + 1} on`);
    toggle.addEventListener('change', () => { setFilter(index, {enabled: toggle.checked}); changed(); commit(); });
    on.append(toggle);

    const number = document.createElement('td'); number.className = 'col-num'; number.textContent = index + 1;

    const typeCell = document.createElement('td');
    const type = document.createElement('select'); type.setAttribute('aria-label', `Type, filter ${index + 1}`);
    for (const [value, text] of Object.entries(TYPES)) type.append(new Option(text, value));
    type.value = f.type;
    type.addEventListener('change', () => {
      const next = type.value, current = state.preset.filters[index];
      const values = {type: next};
      if (isPass(next)) values.gain = 0;
      else if (isPass(current.type)) values.gain = 0;
      if (isShelf(next)) values.q = SHELF_Q;
      else if (isShelf(current.type)) values.q = isPass(next) ? SHELF_Q : 1;
      setFilter(index, values);
      changed({rows: true}); commit();
      $('filter-rows').children[index]?.querySelector('select').focus();
    });
    typeCell.append(type);

    row.append(on, number, typeCell, numberCell(index, 'frequency', 'Hz', {min: 20, max: 20000, stepValue: 1}));
    row.append(isPass(f.type) ? fixedCell('—', 'Pass filters have no gain.') : numberCell(index, 'gain', 'dB', {min: -12, max: 12, stepValue: 0.1}));
    row.append(isShelf(f.type) ? fixedCell('S = 1', 'Shelf slope is fixed, so Q does not apply.') : numberCell(index, 'q', '', {min: 0.1, max: 20, stepValue: 0.01}));

    const removeCell = document.createElement('td'); removeCell.className = 'col-remove';
    const remove = document.createElement('button');
    remove.type = 'button'; remove.className = 'icon-button'; remove.setAttribute('aria-label', `Remove filter ${index + 1}`);
    remove.innerHTML = '<svg><use href="#i-close"/></svg>';
    remove.addEventListener('click', () => removeFilter(index));
    removeCell.append(remove);
    row.append(removeCell);
    body.append(row);
  });
  renderRowState({values: true});
}

function renderRowState({values = false} = {}) {
  const issues = problems();
  const rows = $('filter-rows').children;
  state.preset.filters.forEach((f, index) => {
    const row = rows[index]; if (!row) return;
    row.classList.toggle('selected', index === state.selected);
    row.classList.toggle('off', !f.enabled);
    row.querySelector('.col-remove button').disabled = state.preset.filters.length <= 1;
    for (const input of row.querySelectorAll('input[data-field]')) {
      const field = input.dataset.field;
      if (values || document.activeElement !== input) {
        const value = f[field];
        const text = value === null || value === undefined ? '' : String(field === 'q' ? round(value, 3) : round(value, field === 'gain' ? 2 : 1));
        if (input.value !== text) input.value = text;
      }
      input.setAttribute('aria-invalid', issues.some(p => p.index === index && p.field === field) ? 'true' : 'false');
    }
  });
}

/* ---------- graph ---------- */
let graph;
function compiledPoints() {
  const compiled = state.compiled;
  if (!compiled || compiled.signature !== signature()) return null;
  const curve = compiled.result.curves.find(c => c.rate === 48000) || compiled.result.curves[0];
  return curve ? curve.points.map(([f, db]) => [f, db - compiled.result.preamp]) : null;
}

function renderGraph() {
  const issues = problems();
  graph.render({filters: payload().filters, selected: state.selected, compiled: compiledPoints(),
    invalid: issues.length > 0 || (!!state.compileError && state.compileError.signature === signature())});
}

/* ---------- compile ---------- */
let compileTimer = null;
function compileSoon(delay = 300) {
  clearTimeout(compileTimer);
  compileTimer = setTimeout(compileNow, delay);
}

function compileNow() {
  clearTimeout(compileTimer);
  const sig = signature();
  if (problems().length) { renderEditorState(); return Promise.resolve(null); }
  if (state.compiled?.signature === sig) { maybeAutoApply(); return Promise.resolve(state.compiled); }
  const request = payload();
  return queue(async () => {
    if (signature() !== sig) return null;
    try {
      const result = await api('/api/compile', request);
      state.compiled = {signature: sig, result};
      if (state.compileError?.signature === sig) state.compileError = null;
    } catch (error) {
      state.compileError = {signature: sig, message: error.message};
    }
    if (signature() === sig) { renderGraph(); renderEditorState(); renderDevice(); maybeAutoApply(); }
    return state.compiled?.signature === sig ? state.compiled : null;
  });
}

/* ---------- device ---------- */
const status = () => state.status || {};
function deviceReady() {
  const s = status();
  return !!(s.live_enabled && s.backup_exists && !s.audeze_running && !state.transportDirty
    && s.live_state !== 'uncertain' && s.storage_state !== 'uncertain');
}
const liveSignature = () => status().live_state === 'applied' ? canon(status().last_profile) : null;
const savedSignature = () => status().storage_state === 'saved' ? canon(status().saved_profile) : null;

function maybeAutoApply() {
  const sig = signature();
  if (!$('auto-apply').checked || !deviceReady() || state.busy || state.compiled?.signature !== sig) return;
  if (liveSignature() === sig || state.attempted.has(sig)) return;
  state.attempted.add(sig);
  deviceOp('apply', {quiet: true});
}

const DEVICE_OPS = {
  connect: {path: '/api/read_device', label: 'Reading…'},
  apply: {path: '/api/apply', label: 'Applying…'},
  save: {path: '/api/save', label: 'Saving…'},
  restore: {path: '/api/restore', label: 'Restoring…'},
  restoreSaved: {path: '/api/restore_saved', label: 'Restoring…'},
};

async function deviceOp(kind, {quiet = false} = {}) {
  const op = DEVICE_OPS[kind];
  const sends = kind === 'apply' || kind === 'save';
  let request = {};
  if (sends) {
    const ready = await compileNow();
    if (!ready) { if (!quiet) toast(state.compileError?.message || 'Fix the highlighted filters first.', 'error'); return; }
    request = payload();
    if (canon(request) !== ready.signature) return;
  } else if (kind === 'connect') request = {transport: $('transport').value};
  return queue(async () => {
    if (sends && canon(request) !== signature()) return;
    state.busy = kind; renderDevice();
    try {
      const result = await api(op.path, request);
      if (kind === 'connect') {
        state.transportDirty = false;
        toast(result.message, result.backup_exists ? 'success' : 'warn');
      } else if (!quiet) toast(result.message, 'success');
      if (kind === 'restore' || kind === 'restoreSaved') state.attempted.add(signature());
    } catch (error) {
      toast(error.message, 'error');
    } finally {
      state.busy = null;
      await refreshStatus().catch(() => {});
    }
    if (kind === 'connect') maybeAutoApply();
  });
}

function deviceView() {
  const s = status(), sig = signature();
  if (!state.status) return {tone: 'idle', line: 'Connecting to the editor…', detail: ''};
  if (!s.live_enabled) return {tone: 'idle', line: 'Offline', detail: 'Started with --offline. Edits are previewed, never sent to the headset.'};
  if (s.audeze_running) return {tone: 'warn', line: 'Paused: Audeze App/HQ is open',
    detail: 'Custom PEQ will not work while it is open. Close it to use the headset.', audeze: true};
  if (s.storage_state === 'uncertain') return {tone: 'error', line: 'The last save was not confirmed',
    detail: 'Open Restore and choose Restore saved preset before sending more EQ.'};
  if (s.live_state === 'uncertain') return {tone: 'error', line: 'The last change was not confirmed',
    detail: 'Open Restore and choose Restore live EQ before sending more EQ.'};
  if (state.transportDirty) return {tone: 'idle', line: 'Connection changed', detail: 'Read the headset again through this connection before sending EQ.'};
  if (!s.backup_exists) return {tone: 'idle', line: 'Headset not read yet',
    detail: 'Connect & back up saves your current custom preset so it can always be restored.'};
  const live = liveSignature(), saved = savedSignature();
  const device = s.device || 'Audeze Maxwell';
  if (live === sig) return {tone: 'ok', line: device, detail: saved === sig ? 'Playing this profile. It is also saved on the headset.' : 'Playing this profile live. Headset storage is unchanged.'};
  if (s.live_state === 'applied') {
    const playing = state.library.profiles.find(p => canon(p.preset) === live);
    return {tone: 'ok', line: device, detail: playing ? `Playing “${playing.name}”. This profile differs.` : 'Playing an earlier version of this profile.'};
  }
  if (s.storage_state === 'saved') return {tone: 'ok', line: device, detail: 'Headset is playing its saved custom preset.'};
  return {tone: 'ok', line: device, detail: 'Headset is playing your original preset. Backup ready.'};
}

function renderDevice() {
  const s = status(), view = deviceView(), sig = signature();
  $('status-dot').dataset.tone = view.tone;
  $('device-line').textContent = view.line;
  $('device-detail').textContent = view.detail;
  const live = !!s.live_enabled, busy = !!state.busy, readable = s.storage_state !== 'uncertain';
  const blocked = !live || s.audeze_running;
  const current = state.compiled?.signature === sig && !problems().length;
  const uncertain = s.live_state === 'uncertain' || s.storage_state === 'uncertain';

  $('transport').disabled = busy || blocked || !readable;
  if (!state.transportDirty && s.transport) $('transport').value = s.transport;
  $('connect').hidden = !live;
  $('connect').disabled = busy || blocked || !readable;
  $('connect').textContent = state.busy === 'connect' ? DEVICE_OPS.connect.label : s.backup_exists && !state.transportDirty ? 'Read again' : 'Connect & back up';
  $('connect').classList.toggle('primary', !s.backup_exists || state.transportDirty);
  $('connect').title = readable ? '' : 'Restore the saved preset through this connection before switching.';
  for (const id of ['apply', 'save-headset', 'restore-menu-button']) $(id).hidden = !live;
  $('apply').disabled = busy || !deviceReady() || !current || liveSignature() === sig;
  $('apply').textContent = state.busy === 'apply' ? DEVICE_OPS.apply.label : 'Apply';
  $('apply').hidden = !live || $('auto-apply').checked;
  $('save-headset').disabled = busy || !deviceReady() || !current || savedSignature() === sig;
  $('save-headset').textContent = state.busy === 'save' ? DEVICE_OPS.save.label : 'Save to headset…';
  const canRestore = !busy && live && !s.audeze_running && s.backup_exists && !state.transportDirty;
  $('restore-live').disabled = !canRestore || s.storage_state === 'uncertain' || (s.live_state === 'unchanged' && !uncertain);
  $('restore-saved').disabled = !canRestore || s.storage_state === 'unchanged';
  $('restore-menu-button').disabled = $('restore-live').disabled && $('restore-saved').disabled;
  $('restore-menu-button').classList.toggle('attention', uncertain);
  $('auto-apply').closest('.switch').hidden = !live;

  const showBanner = !!s.audeze_running && !state.audezeDismissed && live;
  $('audeze-banner').hidden = !showBanner;
  $('show-audeze').hidden = !(view.audeze && state.audezeDismissed);
  $('open-balance').hidden = !live;
  $('open-headset').hidden = !live;
  renderBalance();
  renderHeadset();
}

/* ---------- balance ---------- */
// Firmware gain steps, not dB. Writes go straight to headset storage.
const BALANCE_MAX = 150, BALANCE_RESERVED = [136, 142];
const SOURCE_LABEL = {'usb-c': 'USB-C', wireless: 'wireless'};
const lastBalance = () => status().balance || null;
const balanceValue = side => { const raw = $(`balance-${side}`).value.trim(); return raw === '' ? NaN : Number(raw); };

function balanceIssue(value) {
  if (!Number.isInteger(value)) return 'Enter whole numbers.';
  if (value < 0 || value > BALANCE_MAX) return `Keep both channels between 0 and ${BALANCE_MAX}.`;
  if (BALANCE_RESERVED.includes(value)) return `${value} is reserved by the firmware. Pick a nearby value.`;
  return '';
}

function setBalanceInputs(values) {
  $('balance-left').value = values.left; $('balance-right').value = values.right;
}

function stepBalance(side, delta) {
  let value = balanceValue(side);
  if (!Number.isInteger(value)) value = lastBalance()?.[side] ?? 0;
  do value += delta; while (BALANCE_RESERVED.includes(value));
  $(`balance-${side}`).value = Math.min(BALANCE_MAX, Math.max(0, value));
  renderBalance();
}

function renderBalance() {
  if (!$('balance-dialog').open) return;
  const s = status(), last = lastBalance(), busy = !!state.busy, mine = state.busy === 'balance';
  const blocked = !s.live_enabled || s.audeze_running;
  const left = balanceValue('left'), right = balanceValue('right');
  const issue = last ? balanceIssue(left) || balanceIssue(right) : '';
  const changed = !!last && (left !== last.left || right !== last.right);

  for (const id of ['balance-left', 'balance-right']) {
    $(id).disabled = !last || busy || blocked;
    $(id).setAttribute('aria-invalid', String(!!balanceIssue(balanceValue(id.slice(8))) && !!last));
  }
  for (const button of $('balance-dialog').querySelectorAll('[data-step]')) button.disabled = !last || busy || blocked;
  $('balance-error').hidden = !issue; $('balance-error').textContent = issue;

  let diff = 'Not read yet';
  if (s.audeze_running) diff = 'Close Audeze App/HQ first';
  else if (last && !issue) {
    const d = right - left;
    diff = d === 0 ? 'Even' : `${d > 0 ? 'Right' : 'Left'} ${Math.abs(d)} step${Math.abs(d) === 1 ? '' : 's'} louder`;
    if (changed) diff += ' · not applied';
  } else if (last) diff = '—';
  $('balance-diff').textContent = diff;

  $('balance-read').disabled = busy || blocked;
  $('balance-read').textContent = mine && state.balanceAction === 'read' ? 'Reading…' : last ? 'Read again' : 'Read headset';
  $('balance-apply').disabled = busy || blocked || !changed || !!issue;
  $('balance-apply').textContent = mine && state.balanceAction === 'apply' ? 'Applying…' : 'Apply balance';
  const first = state.balanceFirst;
  $('balance-revert').hidden = !first || !last || (first.left === last.left && first.right === last.right);
  if (first) $('balance-revert').textContent = `Put back ${first.left} / ${first.right}`;
  $('balance-revert').disabled = busy || blocked;

  const message = state.balanceMessage;
  $('balance-status').textContent = message?.text || '';
  $('balance-status').dataset.kind = message?.kind || '';

  const profile = last?.source_profile;
  $('balance-source-line').textContent = profile ? (profile === 'usb-c' ? 'USB-C' : 'Wireless') : 'Read the headset to see it.';
  const target = profile === 'usb-c' ? 'wireless' : 'usb-c';
  $('balance-source-switch').hidden = !profile;
  $('balance-source-switch').disabled = busy || blocked;
  $('balance-source-switch').textContent = mine && state.balanceAction === 'source' ? 'Switching…' : `Switch to ${SOURCE_LABEL[target]}…`;
  $('balance-persist').textContent = !profile ? ''
    : profile === 'usb-c'
      ? 'An applied balance is saved on the headset and loads at every power-on. A factory reset or a fully drained battery clears it.'
      : 'On this profile the headset loads its stored wireless balance at power-on, so an applied balance may only last until the headset turns off. On the USB-C profile it loads at power-on.';
}

function openBalance() {
  const last = lastBalance();
  if (last) setBalanceInputs(last);
  state.balanceMessage = null;
  $('balance-dialog').showModal();
  renderBalance();
  if (!last && !$('balance-read').disabled) balanceOp('read');
}

// Results stay inside the dialog: toasts sit under its backdrop.
async function balanceOp(action, body = {}) {
  const path = {read: '/api/balance/read', apply: '/api/balance/apply', source: '/api/balance/source'}[action];
  // Writes go through the connection the balance was read through.
  const request = action === 'read' ? {transport: $('transport').value} : body;
  return queue(async () => {
    state.busy = 'balance'; state.balanceAction = action; state.balanceMessage = null; renderDevice();
    try {
      const result = await api(path, request);
      state.balanceFirst ??= result.previous ? {left: result.previous.left, right: result.previous.right} : {left: result.left, right: result.right};
      setBalanceInputs(result);
      state.balanceMessage = {text: result.message, kind: 'success'};
    } catch (error) {
      state.balanceMessage = {text: error.message, kind: 'error'};
    } finally {
      state.busy = null; state.balanceAction = null;
      await refreshStatus().catch(() => {});
      renderDevice();
    }
  });
}

async function switchSource() {
  const target = lastBalance()?.source_profile === 'usb-c' ? 'wireless' : 'usb-c', label = SOURCE_LABEL[target];
  const ok = await confirmDialog({
    title: `Switch the source profile to ${label}?`,
    body: 'The headset reads this once at power-on, so turn it off and on again afterwards. It picks which stored balance loads at power-on, and the firmware keeps separate defaults for each profile. It stays set until you change it here; a factory reset does not change it.',
    ok: `Switch to ${label}`,
  });
  if (ok) balanceOp('source', {source: target});
}

/* ---------- headset controls ---------- */
// Everyday controls from Audeze HQ. Each change goes to the headset at once and is read back.
const lastControls = () => status().controls || null;
const NOISE_LABEL = ['off', 'low', 'high'];
const CONTROL_LABEL = {
  chat_mix: 'Game / chat mix', volume_limiter: 'Volume limiter', sidetone: 'Sidetone',
  sidetone_level: 'Sidetone level', noise_reduction: 'Noise suppression',
  voice_prompt: 'Voice prompt volume', auto_off: 'Auto shutdown',
};
const sidetonePercent = (raw, max) => max ? `${Math.round(100 * raw / max)}%` : '—';
const voicePercent = v => v === 0 ? '0%' : `${Math.round((v + 1) * 6.25)}%`;
const chatMixText = v => v === 10 ? 'Even' : v < 10 ? `Game +${10 - v}` : `Chat +${v - 10}`;
function autoOffText(seconds) {
  const option = [...$('hs-auto-off').options].find(o => Number(o.value) === seconds);
  return option ? option.textContent : `${Math.round(seconds / 60)} min`;
}

function controlText(control, value) {
  switch (control) {
    case 'chat_mix': return chatMixText(value);
    case 'volume_limiter': case 'sidetone': return value ? 'on' : 'off';
    case 'sidetone_level': return sidetonePercent(value, lastControls()?.sidetone_max);
    case 'noise_reduction': return NOISE_LABEL[value] ?? String(value);
    case 'voice_prompt': return voicePercent(value);
    case 'auto_off': return autoOffText(value);
    default: return String(value);
  }
}

// Inputs follow the last reading. Rendering leaves them alone so a status poll can't undo a drag.
function setHeadsetInputs(c) {
  if (!c) return;
  if (c.preset?.index) $('hs-preset').value = c.preset.index;
  $('hs-chat-mix').value = c.chat_mix;
  $('hs-limiter').checked = !!c.volume_limiter;
  $('hs-sidetone').checked = !!c.sidetone;
  $('hs-sidetone-level').max = c.sidetone_max || 0;
  $('hs-sidetone-level').value = c.sidetone_level;
  for (const radio of document.getElementsByName('hs-noise')) radio.checked = Number(radio.value) === c.noise_reduction;
  $('hs-auto-off').value = String(c.auto_off);
  $('hs-voice-prompt').value = c.voice_prompt;
  renderHeadsetValues();
}

function renderHeadsetValues() {
  const c = lastControls();
  $('hs-chat-mix-value').textContent = c ? chatMixText(Number($('hs-chat-mix').value)) : '—';
  $('hs-sidetone-level-value').textContent = c ? sidetonePercent(Number($('hs-sidetone-level').value), c.sidetone_max) : '—';
  $('hs-voice-prompt-value').textContent = c ? voicePercent(Number($('hs-voice-prompt').value)) : '—';
}

function renderHeadset() {
  if (!$('headset-dialog').open) return;
  const s = status(), c = lastControls(), busy = !!state.busy, mine = state.busy === 'headset';
  const blocked = !s.live_enabled || s.audeze_running;
  const off = !c || busy || blocked;

  $('hs-battery').textContent = c?.battery != null ? `${c.battery}%` : '—';
  $('hs-mic').textContent = c?.mic_muted == null ? '—' : c.mic_muted ? 'Muted' : 'On';
  const firmware = c?.firmware;
  $('hs-firmware').textContent = !firmware?.headset ? '—'
    : firmware.dongle ? `${firmware.headset} · dongle ${firmware.dongle}` : firmware.headset;

  for (const id of ['hs-chat-mix', 'hs-limiter', 'hs-sidetone', 'hs-auto-off', 'hs-voice-prompt']) $(id).disabled = off;
  $('hs-sidetone-level').disabled = off || !c.sidetone_max;
  for (const radio of document.getElementsByName('hs-noise')) radio.disabled = off;

  // The headset loads the new preset's own EQ, so EQ sent from here has to be put back first.
  const eqSent = s.live_state !== 'unchanged' || s.storage_state !== 'unchanged';
  $('hs-preset').disabled = off || eqSent;
  $('hs-preset-note').textContent = !c ? ''
    : eqSent ? 'Restore the original EQ before switching. It belongs to the preset that is active now.'
      : c.preset?.custom ? 'Switching reads the new preset and backs up its EQ.'
        : 'Built-in presets can’t be edited. Switch to EQ 1 to EQ 4 to send your own EQ.';

  $('headset-read').disabled = busy || blocked;
  $('headset-read').textContent = mine && state.headsetAction === 'read' ? 'Reading…' : c ? 'Read again' : 'Read headset';
  const message = s.audeze_running && !mine ? {text: 'Close Audeze App/HQ to change these here.', kind: 'error'} : state.headsetMessage;
  $('headset-status').textContent = message?.text || '';
  $('headset-status').dataset.kind = message?.kind || '';
  renderHeadsetValues();
}

function openHeadset() {
  state.headsetMessage = null;
  setHeadsetInputs(lastControls());
  $('headset-dialog').showModal();
  renderHeadset();
  if (!lastControls() && !$('headset-read').disabled) headsetOp('read');
}

// Results stay inside the dialog: toasts sit under its backdrop.
async function headsetOp(action, body = {}) {
  const path = {read: '/api/controls/read', set: '/api/controls/set', preset: '/api/preset/select'}[action];
  // Control writes go through the connection the controls were read through.
  const request = action === 'set' ? body : {...body, transport: $('transport').value};
  return queue(async () => {
    state.busy = 'headset'; state.headsetAction = action; state.headsetMessage = null; renderDevice();
    try {
      const result = await api(path, request);
      let text = '';
      if (action === 'preset') { state.transportDirty = false; text = result.message; }
      else if (action === 'set') {
        const label = CONTROL_LABEL[body.control], shown = controlText(body.control, result.value);
        text = result.changed ? `${label} set to ${shown}.` : `${label} was already ${shown}.`;
      }
      state.headsetMessage = text ? {text, kind: 'success'} : null;
    } catch (error) {
      state.headsetMessage = {text: error.message, kind: 'error'};
    } finally {
      state.busy = null; state.headsetAction = null;
      await refreshStatus().catch(() => {});
      setHeadsetInputs(lastControls());
      renderDevice();
    }
  });
}

const setControl = (control, value) => headsetOp('set', {control, value});

let statusTimer = null;
async function refreshStatus() {
  const result = await api('/api/status');
  const wasRunning = state.status?.audeze_running, encoder = state.status?.encoder;
  state.status = result; state.token = result.token;
  if (wasRunning && !result.audeze_running) state.audezeDismissed = false;
  if (result.session_recovered && !state.recoveryShown && (result.live_state !== 'unchanged' || result.storage_state !== 'unchanged')) {
    state.recoveryShown = true;
    toast('Recovered the last session. Your original preset backup is still available under Restore.', 'info');
  }
  if (state.preset) {
    renderDevice(); renderProfiles(); maybeAutoApply();
    if (encoder !== result.encoder) renderEditorState();
  }
  return result;
}
function pollStatus() {
  clearTimeout(statusTimer);
  statusTimer = setTimeout(async () => {
    if (!document.hidden) await refreshStatus().catch(() => {
      $('status-dot').dataset.tone = 'error';
      $('device-line').textContent = 'Lost contact with the editor';
      $('device-detail').textContent = 'Restart Maxwell PEQ.';
    });
    pollStatus();
  }, 2500);
}

/* ---------- rendering ---------- */
function renderProfiles() {
  const live = liveSignature(), saved = savedSignature();
  const rows = state.library.profiles.map((p, i) => {
    const sig = p.id === state.library.active ? signature() : canon(p.preset);
    return {p, i, live: !!live && sig === live, saved: !!saved && sig === saved};
  });
  const key = JSON.stringify([state.library.active, rows.map(r => [r.p.id, r.p.name, r.live, r.saved])]);
  if (key === state.profileKey) return;
  state.profileKey = key;
  const list = $('profile-list');
  const focused = document.activeElement?.closest?.('.profile-item')?.dataset.id;
  list.replaceChildren();
  for (const {p, i, live: isLive, saved: isSaved} of rows) {
    const item = document.createElement('li');
    item.className = 'profile-item'; item.dataset.id = p.id;
    const button = document.createElement('button');
    button.type = 'button'; button.className = 'profile-button';
    if (p.id === state.library.active) button.setAttribute('aria-current', 'true');
    const name = document.createElement('span'); name.className = 'profile-label'; name.textContent = p.name;
    button.append(name);
    if (isLive) { const badge = document.createElement('span'); badge.className = 'badge live'; badge.textContent = 'Live'; button.append(badge); }
    if (isSaved) { const badge = document.createElement('span'); badge.className = 'badge saved'; badge.textContent = 'Saved'; button.append(badge); }
    if (i < 9) { const key = document.createElement('kbd'); key.textContent = `Ctrl+${i + 1}`; key.setAttribute('aria-hidden', 'true'); button.append(key); }
    button.addEventListener('click', () => activate(p.id));
    const more = document.createElement('button');
    more.type = 'button'; more.className = 'icon-button more';
    more.setAttribute('aria-label', `More actions for ${p.name}`); more.setAttribute('aria-haspopup', 'menu');
    more.innerHTML = '<svg><use href="#i-more"/></svg>';
    more.addEventListener('click', event => { event.stopPropagation(); openProfileMenu(p.id, more); });
    item.append(button, more);
    list.append(item);
  }
  if (focused) list.querySelector(`[data-id="${CSS.escape(focused)}"] .profile-button`)?.focus();
}

function renderEditorState() {
  const history = historyFor(state.library.active);
  const pending = JSON.stringify(state.preset) !== state.committed;
  $('undo').disabled = !history.undo.length && !pending;
  $('redo').disabled = !history.redo.length || pending;
  const count = state.preset.filters.length;
  $('filter-count').textContent = `${count} of ${MAX_FILTERS}`;
  $('add-filter').disabled = count >= MAX_FILTERS;

  const manual = state.preset.preamp !== 'auto';
  $('preamp-mode').value = manual ? 'manual' : 'auto';
  $('preamp-manual').hidden = !manual;
  if (manual && document.activeElement !== $('preamp-value')) $('preamp-value').value = state.preset.preamp ?? '';
  $('preamp-value').setAttribute('aria-invalid', problems().some(p => p.field === 'preamp') ? 'true' : 'false');

  const sig = signature(), compiled = state.compiled?.signature === sig ? state.compiled.result : null;
  const error = state.compileError?.signature === sig ? state.compileError.message : null;
  const issues = problems();
  $('preamp-result').textContent = compiled ? `${manual ? 'Using' : 'Auto'} ${minus(compiled.preamp.toFixed(1))} dB` : '';

  const note = $('type-note');
  const text = issues.length ? issues.map(p => p.text).join(' ') : error;
  note.classList.toggle('error', !!text);
  if (text) note.textContent = text;
  else if (state.preset.filters.some(f => f.enabled && f.type !== 'peak'))
    note.textContent = 'Shelf and pass filters are experimental. Shelves use a fixed slope (S = 1), so Q does not apply.';
  else if (status().encoder === 'built-in')
    note.textContent = status().dll_available
      ? 'Using the built-in encoder (MAXWELL_PEQ_ENCODER=built-in). It has not been compared with the Audeze library by ear yet.'
      : 'Using the built-in encoder because the Audeze PEQ library is not installed. It has not been compared with the Audeze library by ear yet.';
  else note.textContent = '';

  $('graph-source').textContent = issues.length ? 'Estimated response · fix the highlighted values'
    : error ? 'Estimated response · could not compile'
    : compiled ? `Compiled 48 kHz response, before preamp · fit error ≤ ${compiled.max_error_db.toFixed(3)} dB · not measured`
    : 'Estimated response · compiling…';
}

function renderAll() {
  renderRows(); renderGraph(); renderEditorState(); renderProfiles(); renderDevice();
}

/* ---------- menus and dialogs ---------- */
function closeMenus() {
  for (const menu of document.querySelectorAll('.menu')) menu.hidden = true;
  $('restore-menu-button').setAttribute('aria-expanded', 'false');
}

function openMenu(menu, anchor) {
  closeMenus();
  menu.hidden = false;
  if (menu.id === 'profile-menu') {
    const rect = anchor.getBoundingClientRect();
    const top = Math.min(rect.bottom + 4, window.innerHeight - menu.offsetHeight - 8);
    menu.style.left = `${Math.round(rect.left)}px`; menu.style.top = `${Math.round(top)}px`;
  }
  menu.querySelector('button:not(:disabled)')?.focus();
}

function menuKeys(event) {
  const items = [...event.currentTarget.querySelectorAll('button:not(:disabled)')];
  const at = items.indexOf(document.activeElement);
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    items[(at + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus();
  } else if (event.key === 'Escape') {
    event.preventDefault(); const owner = state.menuOwner; closeMenus(); owner?.focus();
  } else if (event.key === 'Tab') closeMenus();
}

function openProfileMenu(id, anchor) {
  state.menuProfile = id; state.menuOwner = anchor;
  $('profile-menu').querySelector('[data-action="delete"]').disabled = state.library.profiles.length <= 1;
  openMenu($('profile-menu'), anchor);
}

function confirmDialog({title, body, ok, danger = false}) {
  const dialog = $('confirm-dialog');
  $('confirm-title').textContent = title;
  $('confirm-body').textContent = body;
  $('confirm-ok').textContent = ok;
  $('confirm-ok').classList.toggle('danger', danger);
  $('confirm-ok').classList.toggle('primary', !danger);
  dialog.returnValue = 'cancel';
  dialog.showModal();
  $('confirm-ok').focus();
  return new Promise(resolve => dialog.addEventListener('close', () => resolve(dialog.returnValue === 'ok'), {once: true}));
}

async function profileAction(action) {
  const id = state.menuProfile, profile = state.library.profiles.find(p => p.id === id);
  closeMenus();
  if (!profile) return;
  if (action === 'rename') {
    await activate(id);
    $('profile-name').focus(); $('profile-name').select();
  } else if (action === 'duplicate') {
    await flushSave();
    const preset = id === state.library.active ? payload() : profile.preset;
    createProfile(`${profile.name} copy`, preset).catch(error => toast(error.message, 'error'));
  } else if (action === 'export') {
    await exportApo(id === state.library.active ? payload() : profile.preset, profile.name, id === state.library.active);
  } else if (action === 'delete') {
    const ok = await confirmDialog({title: `Delete “${profile.name}”?`, body: 'The profile is removed from this computer. Anything already saved on the headset stays there.', ok: 'Delete', danger: true});
    if (!ok) return;
    await flushSave();
    try {
      const result = await api('/api/profiles/delete', {id});
      state.history.delete(id);
      const switching = result.active !== state.library.active;
      adoptLibrary(result);
      if (switching) loadActive(); else renderProfiles();
      toast(`Deleted “${profile.name}”.`);
    } catch (error) { toast(error.message, 'error'); }
  }
}

/* ---------- APO import/export ---------- */
const fileName = name => `${name.replace(/[<>:"/\\|?*\u0000-\u001f]+/g, ' ').trim() || 'preset'}.txt`;

async function apoText(preset, useCompiledPreamp) {
  const compiled = useCompiledPreamp && state.compiled?.signature === signature() ? state.compiled.result : null;
  return (await api('/api/apo/export', {preset, preamp_db: compiled ? compiled.preamp : null})).text;
}

async function exportApo(preset = payload(), name = activeProfile().name, current = true) {
  try {
    if (current) await compileNow();
    const text = await apoText(preset, current);
    if (tauri) {
      const saved = await tauri.core.invoke('save_text', {filename: fileName(name), text});
      if (saved) toast(`Exported to ${saved}`, 'success');
      return;
    }
    const url = URL.createObjectURL(new Blob([text], {type: 'text/plain'}));
    const link = document.createElement('a'); link.href = url; link.download = fileName(name); link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  } catch (error) { toast(`Couldn't export: ${error.message}`, 'error'); }
}

async function copyApo() {
  try {
    await compileNow();
    const text = await apoText(payload(), true);
    try { await navigator.clipboard.writeText(text); }
    catch {
      const area = Object.assign(document.createElement('textarea'), {value: text});
      document.body.append(area); area.select(); document.execCommand('copy'); area.remove();
    }
    toast('Copied as Equalizer APO text.', 'success');
  } catch (error) { toast(`Couldn't copy: ${error.message}`, 'error'); }
}

const importState = {parsed: null, timer: null, request: 0, nameEdited: false};

function openImport(text = '', name = '') {
  const dialog = $('import-dialog');
  $('import-text').value = text;
  $('import-file-name').textContent = name ? name : '';
  importState.nameEdited = false;
  $('import-name').value = uniqueName(name ? name.replace(/\.[^.]+$/, '') : 'Imported preset');
  if (!dialog.open) dialog.showModal();
  parseImport();
  if (!text) $('import-text').focus();
}

function parseImport() {
  clearTimeout(importState.timer);
  const text = $('import-text').value;
  const request = ++importState.request;
  if (!text.trim()) {
    importState.parsed = null; $('import-confirm').disabled = true;
    $('import-result').innerHTML = '<p class="muted">Filters appear here as soon as the text can be read.</p>';
    return;
  }
  importState.timer = setTimeout(async () => {
    try {
      const result = await api('/api/apo/parse', {text});
      if (request !== importState.request) return;
      importState.parsed = result;
      renderImport(result);
    } catch (error) {
      if (request !== importState.request) return;
      importState.parsed = null;
      const message = document.createElement('p'); message.className = 'import-error'; message.textContent = error.message;
      $('import-result').replaceChildren(message);
    }
    $('import-confirm').disabled = !importState.parsed?.filters?.length;
  }, 200);
}

function renderImport(result) {
  const box = $('import-result');
  const table = document.createElement('table'); table.className = 'import-table';
  table.innerHTML = '<thead><tr><th scope="col">#</th><th scope="col">Type</th><th scope="col">Frequency</th><th scope="col">Gain</th><th scope="col">Q</th></tr></thead>';
  const body = document.createElement('tbody');
  result.filters.forEach((f, i) => {
    const row = document.createElement('tr');
    if (f.enabled === false) row.className = 'off';
    for (const text of [i + 1, TYPES[f.type] + (f.enabled === false ? ' (off)' : ''), fmtHz(f.frequency),
      isPass(f.type) ? '—' : fmtDb(f.gain), isShelf(f.type) ? 'S = 1' : round(f.q, 3)]) {
      const cell = document.createElement('td'); cell.textContent = text; row.append(cell);
    }
    body.append(row);
  });
  table.append(body);
  const summary = document.createElement('p'); summary.className = 'import-summary';
  summary.textContent = `${result.filters.length} ${result.filters.length === 1 ? 'filter' : 'filters'} ready to import.`;
  const parts = [summary, table];
  if (result.warnings?.length) {
    const list = document.createElement('ul'); list.className = 'import-warnings';
    for (const warning of result.warnings) { const item = document.createElement('li'); item.textContent = warning; list.append(item); }
    parts.push(list);
  }
  box.replaceChildren(...parts);
}

async function readFile(file) {
  if (file.size > 200000) throw new Error('That file is too large for an Equalizer APO preset.');
  return file.text();
}

async function confirmImport(event) {
  event.preventDefault();
  const parsed = importState.parsed; if (!parsed?.filters?.length) return;
  const name = $('import-name').value.trim() || 'Imported preset';
  $('import-confirm').disabled = true;
  try {
    await createProfile(name, {filters: parsed.filters, preamp: parsed.preamp ?? 'auto'});
    $('import-dialog').close();
    toast(`Added “${activeProfile().name}” with ${parsed.filters.length} ${parsed.filters.length === 1 ? 'filter' : 'filters'}.`, 'success');
  } catch (error) {
    toast(error.message, 'error');
    $('import-confirm').disabled = false;
  }
}

/* ---------- wiring ---------- */
function wire() {
  graph = PEQGraph.create($('graph'), {
    onSelect: (index, options) => select(index, options),
    onDrag(index, change) {
      setFilter(index, change);
      if (state.selected !== index) state.selected = index;
      changed({compile: false});
    },
    onCommit() { commit(); compileNow(); },
    onRemove: removeFilter,
    onAdd: (frequency, gain) => addFilter(frequency, gain),
  });

  $('add-filter').addEventListener('click', () => addFilter());
  $('undo').addEventListener('click', () => step(-1));
  $('redo').addEventListener('click', () => step(1));
  $('preamp-mode').addEventListener('change', () => {
    state.preset.preamp = $('preamp-mode').value === 'auto' ? 'auto'
      : state.compiled?.signature === signature() ? Math.floor(state.compiled.result.preamp * 10) / 10 : -6;
    changed(); commit();
    if (state.preset.preamp !== 'auto') $('preamp-value').focus();
  });
  $('preamp-value').addEventListener('input', () => {
    state.preset.preamp = $('preamp-value').value === '' ? null : Number($('preamp-value').value);
    changed();
  });
  $('preamp-value').addEventListener('change', commit);

  const name = $('profile-name');
  name.addEventListener('keydown', event => {
    if (event.key === 'Enter') { event.preventDefault(); name.blur(); }
    if (event.key === 'Escape') { name.value = activeProfile().name; name.blur(); }
  });
  name.addEventListener('change', () => {
    const profile = activeProfile(), value = name.value.trim();
    if (!value || value === profile.name) { name.value = profile.name; return; }
    profile.name = value; name.value = value;
    state.profileKey = ''; renderProfiles();
    persist(profile.id, {name: value});
  });

  $('new-profile').addEventListener('click', () => createProfile('New profile', flatPreset())
    .then(() => { $('profile-name').focus(); $('profile-name').select(); })
    .catch(error => toast(error.message, 'error')));

  $('profile-menu').addEventListener('click', event => {
    const action = event.target.closest('[data-action]')?.dataset.action;
    if (action) profileAction(action);
  });
  $('profile-menu').addEventListener('keydown', menuKeys);
  $('restore-menu').addEventListener('keydown', menuKeys);
  $('restore-menu-button').addEventListener('click', event => {
    event.stopPropagation();
    if (!$('restore-menu').hidden) { closeMenus(); return; }
    state.menuOwner = $('restore-menu-button');
    openMenu($('restore-menu'), $('restore-menu-button'));
    $('restore-menu-button').setAttribute('aria-expanded', 'true');
  });
  document.addEventListener('click', event => { if (!event.target.closest('.menu')) closeMenus(); });
  window.addEventListener('blur', closeMenus);

  $('auto-apply').addEventListener('change', () => {
    localStorage.setItem('maxwell-peq.auto-apply', $('auto-apply').checked ? '1' : '0');
    state.attempted.clear(); renderDevice(); maybeAutoApply();
  });
  $('auto-apply').checked = localStorage.getItem('maxwell-peq.auto-apply') !== '0';
  $('transport').addEventListener('change', () => {
    state.transportDirty = $('transport').value !== status().transport;
    renderDevice();
  });
  $('connect').addEventListener('click', () => deviceOp('connect'));
  $('apply').addEventListener('click', () => deviceOp('apply'));
  $('save-headset').addEventListener('click', async () => {
    const ok = await confirmDialog({
      title: `Save “${activeProfile().name}” to the headset?`,
      body: 'This writes the EQ into the headset’s active custom preset and plays it. Your original preset stays backed up; Restore → Restore saved preset puts it back. Audeze HQ will still show its old slider values, and moving them there replaces this EQ.',
      ok: 'Save to headset',
    });
    if (ok) deviceOp('save');
  });
  $('restore-live').addEventListener('click', () => { closeMenus(); deviceOp('restore'); });
  $('restore-saved').addEventListener('click', async () => {
    closeMenus();
    const ok = await confirmDialog({title: 'Restore the saved preset?', body: 'This writes your original preset back into the headset’s custom preset slot and plays it.', ok: 'Restore saved preset'});
    if (ok) deviceOp('restoreSaved');
  });

  $('close-audeze').addEventListener('click', async () => {
    const button = $('close-audeze');
    button.disabled = true; button.textContent = 'Closing…';
    try {
      await api('/api/close_audeze', {});
      toast('Closed Audeze App/HQ.', 'success');
    } catch (error) { toast(error.message, 'error'); }
    finally {
      button.disabled = false; button.textContent = 'Yes';
      await refreshStatus().catch(() => {});
    }
  });
  $('keep-audeze').addEventListener('click', () => { state.audezeDismissed = true; renderDevice(); $('show-audeze').focus(); });
  $('show-audeze').addEventListener('click', () => { state.audezeDismissed = false; renderDevice(); $('close-audeze').focus(); });

  $('copy-apo').addEventListener('click', copyApo);
  $('export-apo').addEventListener('click', () => exportApo());

  $('open-import').addEventListener('click', () => openImport());
  $('import-text').addEventListener('input', () => { $('import-file-name').textContent = ''; parseImport(); });
  $('import-name').addEventListener('input', () => { importState.nameEdited = true; });
  $('import-choose').addEventListener('click', () => $('import-file').click());
  $('import-file').addEventListener('change', async event => {
    const file = event.target.files[0]; event.target.value = '';
    if (!file) return;
    try {
      $('import-text').value = await readFile(file);
      $('import-file-name').textContent = file.name;
      if (!importState.nameEdited) $('import-name').value = uniqueName(file.name.replace(/\.[^.]+$/, ''));
      parseImport();
    } catch (error) { toast(error.message, 'error'); }
  });
  $('import-form').addEventListener('submit', confirmImport);
  for (const button of document.querySelectorAll('[data-close]')) button.addEventListener('click', () => button.closest('dialog').close());

  $('open-balance').addEventListener('click', openBalance);
  $('balance-read').addEventListener('click', () => balanceOp('read'));
  $('balance-form').addEventListener('submit', event => {
    event.preventDefault();
    if (!$('balance-apply').disabled) balanceOp('apply', {left: balanceValue('left'), right: balanceValue('right')});
  });
  $('balance-revert').addEventListener('click', () => {
    const first = state.balanceFirst;
    setBalanceInputs(first);
    balanceOp('apply', {left: first.left, right: first.right});
  });
  $('balance-source-switch').addEventListener('click', switchSource);

  $('open-headset').addEventListener('click', openHeadset);
  $('headset-read').addEventListener('click', () => headsetOp('read'));
  $('headset-form').addEventListener('submit', event => event.preventDefault());
  $('hs-preset').addEventListener('change', () => headsetOp('preset', {index: Number($('hs-preset').value)}));
  for (const id of ['hs-chat-mix', 'hs-sidetone-level', 'hs-voice-prompt']) $(id).addEventListener('input', renderHeadsetValues);
  $('hs-chat-mix').addEventListener('change', () => setControl('chat_mix', Number($('hs-chat-mix').value)));
  $('hs-sidetone-level').addEventListener('change', () => setControl('sidetone_level', Number($('hs-sidetone-level').value)));
  $('hs-voice-prompt').addEventListener('change', () => setControl('voice_prompt', Number($('hs-voice-prompt').value)));
  $('hs-limiter').addEventListener('change', () => setControl('volume_limiter', $('hs-limiter').checked));
  $('hs-sidetone').addEventListener('change', () => setControl('sidetone', $('hs-sidetone').checked));
  $('hs-auto-off').addEventListener('change', () => setControl('auto_off', Number($('hs-auto-off').value)));
  for (const radio of document.getElementsByName('hs-noise')) {
    radio.addEventListener('change', () => { if (radio.checked) setControl('noise_reduction', Number(radio.value)); });
  }
  for (const id of ['balance-left', 'balance-right']) $(id).addEventListener('input', renderBalance);
  for (const button of $('balance-dialog').querySelectorAll('[data-step]')) {
    const [side, delta] = button.dataset.step.split(':');
    button.addEventListener('click', () => stepBalance(side, Number(delta)));
  }

  let dragDepth = 0;
  const hasFiles = event => [...(event.dataTransfer?.types || [])].includes('Files');
  window.addEventListener('dragenter', event => { if (!hasFiles(event)) return; dragDepth++; $('drop-overlay').hidden = false; });
  window.addEventListener('dragleave', event => { if (!hasFiles(event)) return; if (--dragDepth <= 0) { dragDepth = 0; $('drop-overlay').hidden = true; } });
  window.addEventListener('dragover', event => { if (hasFiles(event)) event.preventDefault(); });
  window.addEventListener('drop', async event => {
    if (!hasFiles(event)) return;
    event.preventDefault(); dragDepth = 0; $('drop-overlay').hidden = true;
    const file = event.dataTransfer.files[0]; if (!file) return;
    try { openImport(await readFile(file), file.name); } catch (error) { toast(error.message, 'error'); }
  });

  document.addEventListener('keydown', event => {
    if (document.querySelector('dialog[open]')) return;
    const ctrl = event.ctrlKey || event.metaKey;
    const typing = event.target.matches?.('input[type="text"], input:not([type]), textarea');
    if (ctrl && /^[1-9]$/.test(event.key)) {
      const profile = state.library.profiles[Number(event.key) - 1];
      if (profile) { event.preventDefault(); activate(profile.id); }
    } else if (ctrl && !typing && (event.key === 'z' || event.key === 'Z')) {
      event.preventDefault(); step(event.shiftKey ? 1 : -1);
    } else if (ctrl && !typing && event.key === 'y') {
      event.preventDefault(); step(1);
    } else if (ctrl && event.key === 'o') {
      event.preventDefault(); openImport();
    } else if (event.key === 'Escape') closeMenus();
  });

  if (tauri) {
    // The window waits for this before closing; don't let a stuck save keep it open.
    tauri.window.getCurrentWindow().onCloseRequested(() => Promise.race([flushSave(), new Promise(done => setTimeout(done, 3000))]));
  } else window.addEventListener('pagehide', () => flushSave(true));
  document.addEventListener('visibilitychange', () => { if (!document.hidden) refreshStatus().catch(() => {}); });
}

async function start() {
  wire();
  try {
    await refreshStatus();
    adoptLibrary(await api('/api/profiles'));
    loadActive();
  } catch (error) {
    $('status-dot').dataset.tone = 'error';
    $('device-line').textContent = 'The editor could not start';
    $('device-detail').textContent = error.message;
    return;
  }
  pollStatus();
}

start();
