'use strict';
const $ = id => document.getElementById(id);
const {SHELF_Q, isPass, isShelf} = PEQGraph;
const TYPES = {peak: 'Peak', low_shelf: 'Low shelf', high_shelf: 'High shelf', low_pass: 'Low-pass', high_pass: 'High-pass'};
const MAX_FILTERS = 10;

const state = {
  token: null, status: null, busy: null,
  library: {profiles: [], active: null}, preset: null, selected: 0,
  history: new Map(), committed: null,
  compiled: null, compileError: null,
  audezeDismissed: false, profileKey: '', slotKey: '',
};

/* ---------- formatting ---------- */
const round = (value, digits) => Math.round(value * 10 ** digits) / 10 ** digits;
// Show at least `min` decimals so a column of values lines up, and up to `max` when the value needs them.
const fixed = (value, min, max) => {
  const text = String(round(value, max)), dot = text.indexOf('.');
  const decimals = dot < 0 ? 0 : text.length - dot - 1;
  return decimals >= min ? text : round(value, max).toFixed(min);
};
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
    q: f.q ?? (isShelf(type) ? SHELF_Q : null), enabled: f.enabled !== false};
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
    if (!inRange(f.q, 0.1, 20)) list.push({index: i, field: 'q', text: `Filter ${i + 1}: Q must be 0.1 to 20.`});
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
      // Moving between shelves keeps Q; entering or leaving them resets it to each type's default.
      if (isShelf(next) && !isShelf(current.type)) values.q = SHELF_Q;
      else if (isShelf(current.type) && !isShelf(next)) values.q = isPass(next) ? SHELF_Q : 1;
      setFilter(index, values);
      changed({rows: true}); commit();
      $('filter-rows').children[index]?.querySelector('select').focus();
    });
    typeCell.append(type);

    row.append(on, number, typeCell, numberCell(index, 'frequency', 'Hz', {min: 20, max: 20000, stepValue: 1}));
    row.append(isPass(f.type) ? fixedCell('—', 'Pass filters have no gain.') : numberCell(index, 'gain', 'dB', {min: -12, max: 12, stepValue: 0.1}));
    row.append(numberCell(index, 'q', '', {min: 0.1, max: 20, stepValue: 0.01}));

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
        const text = value === null || value === undefined ? '' : field === 'q' ? fixed(value, 2, 3) : field === 'gain' ? fixed(value, 1, 2) : String(round(value, 1));
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
  if (state.compiled?.signature === sig) return Promise.resolve(state.compiled);
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
    if (signature() === sig) { renderGraph(); renderEditorState(); }
    return state.compiled?.signature === sig ? state.compiled : null;
  });
}

/* ---------- device ---------- */
const status = () => state.status || {};
// Custom presets are indexes 7 to 10; the Audeze app calls them Preset 1 to 4.
const FIRST_CUSTOM = 7;
const PROFILE_TYPE = 'application/x-maxwell-profile';
const customActive = () => status().index >= FIRST_CUSTOM && status().index <= FIRST_CUSTOM + 3;
const slotList = () => status().slots || [];
const presetName = slot => `Preset ${slot + 1}`;
const headsetBlocked = () => !status().live_enabled || !!status().audeze_running;
const profileSignature = profile => profile.id === state.library.active ? signature() : canon(profile.preset);

// What a preset holds, as far as this app knows.
function slotView(slot) {
  const profile = slot.profile_id ? state.library.profiles.find(p => p.id === slot.profile_id) : null;
  return {
    slot: slot.slot, active: !!slot.active, linked: !!slot.profile_id,
    name: profile?.name ?? slot.name ?? null, profileId: profile?.id ?? null,
    edited: !!profile && !!slot.preset && profileSignature(profile) !== canon(slot.preset),
  };
}

// Device requests run one at a time; each ends with a fresh status.
function headsetTask(busy, path, body, {quiet = false} = {}) {
  return queue(async () => {
    state.busy = busy; renderDevice();
    try {
      const result = await api(path, {...body, transport: $('transport').value});
      if (!quiet) toast(result.message, 'success');
      return result;
    } catch (error) {
      toast(error.message, 'error');
      return null;
    } finally {
      state.busy = null;
      await refreshStatus().catch(() => {});
      renderDevice();
    }
  });
}

async function readHeadset({quiet = false} = {}) {
  const result = await headsetTask('connect', '/api/read_device', {}, {quiet: true});
  if (result && (!quiet || !result.custom)) toast(result.message, result.custom ? 'success' : 'warn');
}

async function saveToSlot(id, slot) {
  const profile = state.library.profiles.find(p => p.id === id);
  if (!profile || state.busy || headsetBlocked()) return;
  await flushSave();
  const preset = payload(id === state.library.active ? state.preset : profile.preset);
  const issues = problems(preset);
  if (issues.length) { toast(`Fix “${profile.name}” first. ${issues[0].text}`, 'error'); return; }
  await headsetTask(`save:${slot}`, '/api/save', {preset, slot, profile_id: id, name: profile.name});
}

function selectSlot(slot) {
  if (state.busy || headsetBlocked()) return;
  return headsetTask(`select:${slot}`, '/api/select', {index: FIRST_CUSTOM + slot});
}

function deviceView() {
  const s = status();
  if (!state.status) return {tone: 'idle', line: 'Connecting to the editor…', detail: ''};
  if (!s.live_enabled) return {tone: 'idle', line: 'Offline', detail: 'Started with --offline. Edits are previewed, never sent to the headset.'};
  if (s.audeze_running) return {tone: 'warn', line: 'Paused: Audeze App/HQ is open',
    detail: 'Custom PEQ will not work while it is open. Close it to use the headset.', audeze: true};
  if (!s.device_read) return {tone: 'idle', line: 'Headset not read yet', detail: 'Read the headset to see which preset is active.'};
  const device = s.device || 'Original Maxwell';
  if (!customActive()) return {tone: 'warn', line: device, detail: 'This preset is built in. Make Preset 1 to 4 active to hear your profiles.'};
  const active = slotList().find(slot => slot.active);
  const view = active ? slotView(active) : null;
  if (!view?.name) return {tone: 'ok', line: device, detail: 'Playing EQ that was not saved from here.'};
  return {tone: 'ok', line: device, detail: view.edited
    ? `Playing “${view.name}” as it was saved. Later edits are not on the headset.`
    : `Playing “${view.name}”.`};
}

function renderDevice() {
  const s = status(), view = deviceView();
  $('status-dot').dataset.tone = view.tone;
  $('device-line').textContent = view.line;
  $('device-detail').textContent = view.detail;
  $('device-detail').title = view.detail;
  const live = !!s.live_enabled, busy = !!state.busy;
  const blocked = headsetBlocked();

  $('transport').disabled = busy || blocked;
  if (s.transport && state.busy !== 'connect') $('transport').value = s.transport;
  $('transport-wrap').hidden = !live;
  $('connect').hidden = !live;
  $('connect').disabled = busy || blocked;
  $('connect').textContent = state.busy === 'connect' ? 'Reading…' : s.device_read ? 'Read again' : 'Read headset';
  $('connect').classList.toggle('primary', !s.device_read);

  const showBanner = !!s.audeze_running && !state.audezeDismissed && live;
  $('audeze-banner').hidden = !showBanner;
  $('show-audeze').hidden = !(view.audeze && state.audezeDismissed);
  $('open-balance').hidden = !live;
  renderSlots();
  $('open-headset').hidden = !live;
  renderBalance();
  renderHeadset();
}

// A profile box: click to edit it, drag it onto a preset to save it there.
function profileChip(p) {
  const chip = document.createElement('div');
  chip.className = 'chip'; chip.dataset.id = p.id;
  const button = document.createElement('button');
  button.type = 'button'; button.className = 'chip-button';
  if (p.id === state.library.active) button.setAttribute('aria-current', 'true');
  const name = document.createElement('span'); name.className = 'chip-label'; name.textContent = p.name;
  button.append(name);
  button.title = status().live_enabled ? `${p.name}. Click to edit, or drag onto a preset to save it there.` : p.name;
  button.addEventListener('click', () => activate(p.id));
  button.draggable = true;
  button.addEventListener('dragstart', event => {
    event.dataTransfer.setData(PROFILE_TYPE, p.id);
    event.dataTransfer.effectAllowed = 'copy';
    document.body.classList.add('dragging-profile');
  });
  button.addEventListener('dragend', () => document.body.classList.remove('dragging-profile'));
  const more = document.createElement('button');
  more.type = 'button'; more.className = 'icon-button more';
  more.setAttribute('aria-label', `More actions for ${p.name}`); more.setAttribute('aria-haspopup', 'menu');
  more.innerHTML = '<svg><use href="#i-more"/></svg>';
  more.addEventListener('click', event => { event.stopPropagation(); openProfileMenu(p.id, more); });
  chip.append(button, more);
  return chip;
}

function renderSlots() {
  const s = status(), blocked = headsetBlocked(), locked = blocked || !!state.busy;
  $('slots').hidden = !s.live_enabled;
  const views = slotList().map(slotView);
  const key = JSON.stringify([views, state.busy, blocked, state.library.active]);
  if (key === state.slotKey) return;
  state.slotKey = key;
  const focused = document.activeElement?.closest?.('.slot')?.dataset.slot;

  const list = $('slot-list');
  list.replaceChildren();
  for (const view of views) {
    const item = document.createElement('li');
    item.className = 'slot'; item.dataset.slot = view.slot;
    item.classList.toggle('active', view.active);
    item.classList.toggle('locked', locked);

    const pick = document.createElement('button');
    pick.type = 'button'; pick.className = 'slot-select';
    pick.classList.toggle('pending', state.busy === `select:${view.slot}`);
    const dot = document.createElement('span'); dot.className = 'slot-dot'; dot.setAttribute('aria-hidden', 'true');
    pick.append(dot, presetName(view.slot));
    pick.setAttribute('aria-pressed', String(view.active));
    pick.title = view.active ? `${presetName(view.slot)} is active` : `Make ${presetName(view.slot)} active`;
    pick.disabled = locked && !view.active;
    pick.addEventListener('click', () => { if (!view.active) selectSlot(view.slot); });

    const link = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    link.classList.add('slot-link'); link.setAttribute('aria-hidden', 'true');
    link.innerHTML = '<use href="#i-link"/>';
    link.classList.toggle('empty', !view.name);

    const target = document.createElement('div'); target.className = 'slot-target';
    const profile = view.profileId && state.library.profiles.find(p => p.id === view.profileId);
    if (state.busy === `save:${view.slot}`) target.append(Object.assign(document.createElement('span'), {className: 'slot-empty', textContent: 'Saving…'}));
    else if (profile) target.append(profileChip(profile));
    else if (view.name) target.append(Object.assign(document.createElement('span'), {className: 'slot-empty', textContent: view.name, title: 'This profile was deleted from this computer.'}));
    else target.append(Object.assign(document.createElement('span'), {className: 'slot-empty', textContent: 'Drop a profile here'}));
    item.append(pick, link, target);

    if (view.edited && state.busy !== `save:${view.slot}`) {
      const note = document.createElement('p'); note.className = 'slot-note';
      note.append('Edited since saving · ');
      const update = document.createElement('button');
      update.type = 'button'; update.className = 'link-button'; update.textContent = 'Update';
      update.setAttribute('aria-label', `Save “${view.name}” to ${presetName(view.slot)} again`);
      update.disabled = locked;
      update.addEventListener('click', () => saveToSlot(view.profileId, view.slot));
      note.append(update);
      item.append(note);
    }

    const accepts = event => [...(event.dataTransfer?.types || [])].includes(PROFILE_TYPE) && !state.busy && !headsetBlocked();
    item.addEventListener('dragover', event => {
      if (!accepts(event)) return;
      event.preventDefault(); event.dataTransfer.dropEffect = 'copy';
      item.classList.add('drop');
    });
    item.addEventListener('dragleave', event => { if (!item.contains(event.relatedTarget)) item.classList.remove('drop'); });
    item.addEventListener('drop', event => {
      item.classList.remove('drop');
      if (!accepts(event)) return;
      event.preventDefault();
      saveToSlot(event.dataTransfer.getData(PROFILE_TYPE), view.slot);
    });
    list.append(item);
  }
  if (focused) list.querySelector(`[data-slot="${focused}"] .slot-select`)?.focus();
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

  // Presets 1 to 4 keep the profiles saved to them; the headset loads each preset's own EQ.
  $('hs-preset').disabled = off;
  $('hs-preset-note').textContent = !c ? ''
    : c.preset?.custom ? 'Switching plays the EQ saved on that preset.'
      : 'Built-in presets can’t be edited. Switch to Preset 1 to 4 to hear your profiles.';

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
      if (action === 'preset') text = result.message;
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
  if (state.preset) {
    renderDevice(); renderProfiles();
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
// With a headset, profiles saved to a preset sit in its row; the rest stay here.
function renderProfiles() {
  const s = status(), live = !!s.live_enabled;
  const linked = new Set(live ? slotList().map(slot => slot.profile_id).filter(Boolean) : []);
  const pool = state.library.profiles.filter(p => !linked.has(p.id));
  const hint = !live ? ''
    : s.audeze_running ? 'Close Audeze App/HQ to change presets.'
    : pool.length ? 'Drag a profile onto a preset to save it there.'
    : 'Every profile is on the headset. Use + to make another.';
  $('slot-hint').hidden = !hint; $('slot-hint').textContent = hint;
  const key = JSON.stringify([live, state.library.active, pool.map(p => [p.id, p.name])]);
  if (key === state.profileKey) return;
  state.profileKey = key;
  $('profiles-heading').textContent = live ? 'Not on the headset' : 'Profiles';
  const list = $('profile-list');
  const focused = document.activeElement?.closest?.('#profile-list .chip')?.dataset.id;
  list.replaceChildren();
  for (const p of pool) {
    const item = document.createElement('li');
    item.append(profileChip(p));
    list.append(item);
  }
  if (focused) list.querySelector(`[data-id="${CSS.escape(focused)}"] .chip-button`)?.focus();
}

/* The compiler moves a value only in the sample-rate banks that can't hold it precisely. */
function describeAdjustments(adjustments) {
  const rates = list => {
    const names = list.map(rate => String(rate / 1000));
    return `${names.length > 1 ? `${names.slice(0, -1).join(', ')} and ${names.at(-1)}` : names[0]} kHz`;
  };
  const groups = new Map();
  for (const change of adjustments) {
    const key = change.encoder ? 'encoder' : `${change.filter}|${change.field}|${change.to}`;
    if (!groups.has(key)) groups.set(key, {change, rates: []});
    groups.get(key).rates.push(change.rate);
  }
  return [...groups.values()].map(({change, rates: list}) => {
    const at = rates(list), one = list.length === 1;
    if (change.encoder) return `At ${at}, the built-in encoder is used because the Audeze library can't hold these filters precisely there.`;
    const value = change.field === 'q' ? `Q ${round(change.to, 3)}` : `${round(change.to, 1)} Hz`;
    const was = change.field === 'q' ? `Q ${round(change.from, 3)}` : `${round(change.from, 1)} Hz`;
    return `Filter ${change.filter + 1} is sent as ${value} instead of ${was} at ${at}, the nearest value ${one ? 'that rate' : 'those rates'} can hold precisely.`;
  }).join(' ');
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
  const adjusted = compiled ? describeAdjustments(compiled.adjustments || []) : '';
  note.classList.toggle('error', !!text);
  if (text) note.textContent = text;
  else if (adjusted) note.textContent = adjusted;
  else if (state.preset.filters.some(f => f.enabled && f.type !== 'peak'))
    note.textContent = 'Shelf and pass filters are experimental. Shelf Q 0.71 gives the standard slope (S = 1); higher Q adds a bump and dip around the shelf frequency.';
  else if (status().encoder === 'built-in')
    note.textContent = status().dll_available
      ? 'Using the built-in encoder (MAXWELL_PEQ_ENCODER=built-in). It has not been compared with the Audeze library by ear yet.'
      : 'Using the built-in encoder because the Audeze PEQ library is not installed. It has not been compared with the Audeze library by ear yet.';
  else note.textContent = '';

  $('graph-source').textContent = issues.length ? 'Estimated response · fix the highlighted values'
    : error ? `Estimated response · not sent: ${error}`
    : compiled ? `Compiled 48 kHz response, before preamp · fit error ≤ ${compiled.max_error_db.toFixed(3)} dB · not measured`
    : 'Estimated response · compiling…';
}

function renderAll() {
  renderRows(); renderGraph(); renderEditorState(); renderProfiles(); renderDevice();
}

/* ---------- menus and dialogs ---------- */
function closeMenus() {
  for (const menu of document.querySelectorAll('.menu')) menu.hidden = true;
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
  for (const item of $('profile-menu').querySelectorAll('[data-action^="slot:"]')) item.disabled = !!state.busy || headsetBlocked();
  for (const item of $('profile-menu').querySelectorAll('[data-action^="slot:"], .menu-separator')) item.hidden = !status().live_enabled;
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
  if (action.startsWith('slot:')) {
    await saveToSlot(id, Number(action.slice(5)));
  } else if (action === 'rename') {
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
      isPass(f.type) ? '—' : fmtDb(f.gain), fixed(f.q, 2, 3)]) {
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

/* ---------- keyboard shortcuts (desktop app only) ---------- */
// Global keys that switch the headset's EQ. While a field records, the app
// releases its shortcuts so their keys reach the page.
const shortcutState = {actions: [], recording: null};
const MODIFIER_CODES = /^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/;

const keysText = keys => (keys || '').replace(/\bSuper\b/g, 'Win');

// KeyA -> A, Digit1 -> 1, Numpad1 -> Num1, ArrowUp -> Up; others keep their code name.
function keyName(code) {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit\d$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-2])$/.test(code)) return code;
  if (code.startsWith('Numpad')) return `Num${code.slice(6)}`;
  if (code.startsWith('Arrow')) return code.slice(5);
  const named = ['Backquote', 'Backslash', 'BracketLeft', 'BracketRight', 'Comma', 'Equal', 'Minus', 'Period', 'Quote',
    'Semicolon', 'Slash', 'Space', 'Enter', 'Tab', 'Home', 'End', 'PageUp', 'PageDown', 'Insert', 'Delete', 'Pause',
    'MediaPlayPause', 'MediaTrackNext', 'MediaTrackPrevious'];
  return named.includes(code) ? code : null;
}

function modifiers(event) {
  return [event.ctrlKey && 'Ctrl', event.altKey && 'Alt', event.shiftKey && 'Shift', event.metaKey && 'Super'].filter(Boolean);
}

function renderShortcuts() {
  $('shortcut-list').replaceChildren(...shortcutState.actions.map(action => {
    const item = document.createElement('li');
    item.className = 'shortcut';
    const name = document.createElement('span');
    name.className = 'shortcut-name';
    name.id = `shortcut-${action.id}`;
    name.textContent = action.label;
    const field = document.createElement('button');
    field.type = 'button';
    field.className = 'shortcut-keys';
    field.dataset.action = action.id;
    field.setAttribute('aria-labelledby', name.id);
    const recording = shortcutState.recording === action.id;
    field.classList.toggle('recording', recording);
    field.classList.toggle('empty', !action.keys && !recording);
    field.textContent = recording ? 'Press keys…' : keysText(action.keys) || 'None';
    field.addEventListener('click', () => record(action.id));
    field.addEventListener('keydown', captureKeys);
    field.addEventListener('blur', () => { if (shortcutState.recording === action.id) stopRecording(); });
    const clear = document.createElement('button');
    clear.type = 'button';
    clear.className = 'icon-button';
    clear.title = `Remove the ${action.label} shortcut`;
    clear.setAttribute('aria-label', clear.title);
    clear.disabled = !action.keys;
    clear.innerHTML = '<svg><use href="#i-close"/></svg>';
    clear.addEventListener('click', () => saveShortcut(action.id, ''));
    item.append(name, field, clear);
    if (action.problem) {
      const problem = document.createElement('p');
      problem.className = 'shortcut-problem';
      problem.textContent = action.problem;
      item.append(problem);
    }
    return item;
  }));
}

function focusShortcut(id) {
  document.querySelector(`.shortcut-keys[data-action="${id}"]`)?.focus();
}

async function record(id) {
  shortcutState.recording = id;
  renderShortcuts();
  focusShortcut(id);
  try { await tauri.core.invoke('shortcuts_pause', {paused: true}); } catch {}
}

async function stopRecording() {
  if (!shortcutState.recording) return;
  shortcutState.recording = null;
  renderShortcuts();
  try { await tauri.core.invoke('shortcuts_pause', {paused: false}); } catch {}
}

function captureKeys(event) {
  const id = shortcutState.recording;
  if (!id || event.target.dataset.action !== id) return;
  event.preventDefault();
  event.stopPropagation();
  const mods = modifiers(event);
  if (!mods.length && event.key === 'Escape') { stopRecording().then(() => focusShortcut(id)); return; }
  if (!mods.length && (event.key === 'Backspace' || event.key === 'Delete')) { saveShortcut(id, ''); return; }
  if (MODIFIER_CODES.test(event.code)) {
    event.target.textContent = `${keysText(mods.join('+'))}+…`;
    return;
  }
  const key = keyName(event.code);
  if (!key) { event.target.textContent = 'That key can’t be used'; return; }
  saveShortcut(id, [...mods, key].join('+'));
}

async function saveShortcut(id, keys) {
  shortcutState.recording = null;
  try {
    shortcutState.actions = (await tauri.core.invoke('shortcuts_set', {shortcuts: {[id]: keys}})).actions;
  } catch (error) { toast(String(error), 'error'); }
  renderShortcuts();
  focusShortcut(id);
}

async function openShortcuts() {
  if (!tauri) return;
  try { shortcutState.actions = (await tauri.core.invoke('shortcuts_get')).actions; }
  catch (error) { toast(String(error), 'error'); return; }
  shortcutState.recording = null;
  renderShortcuts();
  if (!$('shortcuts-dialog').open) $('shortcuts-dialog').showModal();
}

async function resetShortcuts() {
  const defaults = Object.fromEntries(shortcutState.actions.map(action => [action.id, action.default]));
  try { shortcutState.actions = (await tauri.core.invoke('shortcuts_set', {shortcuts: defaults})).actions; }
  catch (error) { toast(String(error), 'error'); }
  renderShortcuts();
}

/* ---------- settings and updates (desktop app only) ---------- */
const settingsState = {values: {}, updates: null, checking: false};

function renderSettings() {
  const values = settingsState.values;
  for (const input of document.querySelectorAll('#settings-dialog [data-setting]')) {
    input.checked = Boolean(values[input.dataset.setting]);
    // Closing and minimizing to the tray need the tray icon.
    const needsTray = ['close_to_tray', 'minimize_to_tray'].includes(input.dataset.setting);
    input.disabled = needsTray && !values.tray_icon;
    input.closest('.setting').classList.toggle('disabled', input.disabled);
  }
  $('settings-version').textContent = values.version ? `Maxwell PEQ ${values.version}` : '';
}

function checkedWhen(seconds) {
  const date = new Date(seconds * 1000);
  const time = date.toLocaleTimeString([], {hour: 'numeric', minute: '2-digit'});
  return date.toDateString() === new Date().toDateString() ? `today at ${time}` : date.toLocaleDateString();
}

function renderUpdates() {
  const result = settingsState.updates;
  const status = $('update-status');
  status.classList.toggle('error', Boolean(result?.error) && !settingsState.checking);
  $('update-check').disabled = settingsState.checking;
  $('update-download').hidden = !result?.update_available;
  $('update-dot').hidden = !result?.update_available;
  if (result?.update_available) $('update-download').textContent = `Download ${result.latest}`;
  if (settingsState.checking) status.textContent = 'Checking…';
  else if (!result) status.textContent = settingsState.values.check_updates ? 'Not checked yet.' : 'Update checks are off.';
  else if (result.error) status.textContent = result.error;
  else if (result.update_available) status.textContent = `Maxwell PEQ ${result.latest} is out. You have ${result.current}.`;
  else status.textContent = `You have the latest version. Checked ${checkedWhen(result.checked_at)}.`;
  renderReleaseNotes(result?.releases || []);
}

function renderReleaseNotes(releases) {
  const list = $('release-notes');
  if (!releases.length) {
    const empty = document.createElement('p');
    empty.className = 'muted';
    empty.textContent = settingsState.updates?.error || !settingsState.updates
      ? 'Check for updates to see the notes for recent releases.' : 'No releases yet.';
    list.replaceChildren(empty);
    return;
  }
  list.replaceChildren(...releases.slice(0, 5).map((release, index) => {
    const details = document.createElement('details');
    details.className = 'release';
    details.open = index === 0;
    const summary = document.createElement('summary');
    summary.innerHTML = '<svg aria-hidden="true"><use href="#i-chevron"/></svg>';
    summary.append(release.version);
    if (release.newer) {
      const badge = document.createElement('span');
      badge.className = 'release-new';
      badge.textContent = 'New';
      summary.append(badge);
    }
    if (release.date) {
      const date = document.createElement('span');
      date.className = 'release-date';
      date.textContent = new Date(`${release.date}T12:00:00`).toLocaleDateString([], {year: 'numeric', month: 'short', day: 'numeric'});
      summary.append(date);
    }
    const body = document.createElement('div');
    body.className = 'release-body';
    body.append(...releaseNotes(release.notes));
    details.append(summary, body);
    return details;
  }));
}

// Release notes are GitHub markdown. Only headings, lists, paragraphs, bold,
// code and link text are kept, built as DOM nodes so nothing in them runs.
// The install instructions under "Downloads" are left out.
function releaseNotes(markdown) {
  const lines = (markdown || '').replace(/<!--[\s\S]*?-->/g, '').split(/\r?\n/);
  const nodes = [];
  let list = null, paragraph = null, skipping = 0;
  const end = () => { list = null; paragraph = null; };
  for (const line of lines) {
    const heading = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (heading && skipping && heading[1].length > skipping) continue;
    if (heading) {
      skipping = /^downloads?$/i.test(heading[2]) ? heading[1].length : 0;
      end();
      if (!skipping && !/^what.s new$/i.test(heading[2])) nodes.push(inline('h4', heading[2]));
      continue;
    }
    if (skipping) continue;
    const item = /^\s*[-*+]\s+(.*)$/.exec(line);
    if (item) {
      paragraph = null;
      if (!list) { list = document.createElement('ul'); nodes.push(list); }
      list.append(inline('li', item[1]));
    } else if (!line.trim()) {
      end();
    } else if (list && /^\s{2,}/.test(line)) {
      list.lastChild.append(' ', ...inline('span', line.trim()).childNodes);
    } else if (paragraph) {
      paragraph.append(' ', ...inline('span', line.trim()).childNodes);
    } else {
      list = null;
      paragraph = inline('p', line.trim());
      nodes.push(paragraph);
    }
  }
  if (!nodes.length) {
    const none = document.createElement('p');
    none.textContent = 'No patch notes for this release.';
    nodes.push(none);
  }
  return nodes;
}

function inline(tag, text) {
  const element = document.createElement(tag);
  for (const part of text.split(/(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\([^)]*\))/)) {
    if (!part) continue;
    let node;
    if (/^\*\*.+\*\*$/.test(part)) { node = document.createElement('strong'); node.textContent = part.slice(2, -2); }
    else if (/^`.+`$/.test(part)) { node = document.createElement('code'); node.textContent = part.slice(1, -1); }
    else if (/^\[.+\]\(.*\)$/.test(part)) node = document.createTextNode(part.slice(1, part.indexOf('](')));
    else node = document.createTextNode(part);
    element.append(node);
  }
  return element;
}

async function openSettings() {
  if (!tauri) return;
  try {
    settingsState.values = await tauri.core.invoke('settings_get');
    settingsState.updates = await tauri.core.invoke('updates_get', {now: false});
  } catch (error) { toast(String(error), 'error'); return; }
  renderSettings();
  renderUpdates();
  if (!$('settings-dialog').open) $('settings-dialog').showModal();
}

async function changeSetting(input) {
  try {
    settingsState.values = await tauri.core.invoke('settings_set', {changes: {[input.dataset.setting]: input.checked}});
  } catch (error) { toast(String(error), 'error'); }
  renderSettings();
  renderUpdates();
}

async function checkForUpdates() {
  settingsState.checking = true;
  renderUpdates();
  try { settingsState.updates = await tauri.core.invoke('updates_get', {now: true}); }
  catch (error) { settingsState.updates = {error: String(error)}; }
  settingsState.checking = false;
  renderUpdates();
}

async function downloadUpdate() {
  try { await tauri.core.invoke('open_release', {url: settingsState.updates.url}); }
  catch (error) { toast(String(error), 'error'); }
}

function openDialog(name) {
  if (name === 'settings') openSettings();
  else if (name === 'shortcuts') openShortcuts();
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
    state.profileKey = ''; renderProfiles(); renderDevice();
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
  document.addEventListener('click', event => { if (!event.target.closest('.menu')) closeMenus(); });
  window.addEventListener('blur', closeMenus);

  localStorage.removeItem('maxwell-peq.auto-apply');
  $('transport').addEventListener('change', () => readHeadset());
  $('connect').addEventListener('click', () => readHeadset());

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
  if (tauri) {
    $('open-shortcuts').hidden = false;
    $('open-shortcuts').addEventListener('click', openShortcuts);
    $('shortcuts-reset').addEventListener('click', resetShortcuts);
    $('shortcuts-dialog').addEventListener('close', stopRecording);
    $('open-settings').hidden = false;
    $('open-settings').addEventListener('click', openSettings);
    for (const input of document.querySelectorAll('#settings-dialog [data-setting]')) {
      input.addEventListener('change', () => changeSetting(input));
    }
    $('settings-shortcuts').addEventListener('click', () => { $('settings-dialog').close(); openShortcuts(); });
    $('update-check').addEventListener('click', checkForUpdates);
    $('update-download').addEventListener('click', downloadUpdate);
    tauri.event.listen('open-dialog', event => openDialog(event.payload));
    tauri.event.listen('updates-changed', event => { settingsState.updates = event.payload; renderUpdates(); });
    tauri.core.invoke('updates_get', {now: false}).then(result => { settingsState.updates = result; renderUpdates(); }, () => {});
  }
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
  if (tauri && location.hash) openDialog(location.hash.slice(1));
  pollStatus();
  const s = status();
  if (s.live_enabled && !s.audeze_running) readHeadset({quiet: true});
}

start();
