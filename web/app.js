'use strict';
const $ = (id) => document.getElementById(id);
const state = {token: null, status: {}, busy: false, compiled: null, signature: null, transportDirty: false, filters: [{type: 'peak', frequency: 1000, gain: -3, q: 1, enabled: true}]};
const filterTypes = {peak: 'Peak', low_shelf: 'Low shelf', high_shelf: 'High shelf', low_pass: 'Low-pass', high_pass: 'High-pass'};
const shelfQ = Math.SQRT1_2;
const isPass = type => type === 'low_pass' || type === 'high_pass';
const isShelf = type => type === 'low_shelf' || type === 'high_shelf';
const curveColors = ['#195fa5', '#ae5c26', '#337557', '#784a91'];

function showMessage(text, error = false) {
  $('message').textContent = text;
  $('message').className = error ? 'error' : '';
  $('message').hidden = false;
}
function download(name, data) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2) + '\n'], {type: 'application/json'}));
  const link = document.createElement('a'); link.href = url; link.download = name; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
function payload() {
  return {filters: state.filters.map(f => ({...f})), preamp: $('preamp-mode').value === 'auto' ? 'auto' : Number($('preamp-value').value)};
}
function isCurrent() { return state.compiled && state.signature === JSON.stringify(payload()); }
function setBusy(busy) { state.busy = busy; updateControls(); }
function updateControls() {
  const current = isCurrent();
  const deviceAllowed = !!state.status.live_enabled && !!state.token;
  const storage = state.status.storage_state || 'unchanged';
  const untouched = state.status.live_state === 'unchanged' && storage === 'unchanged';
  const uncertain = state.status.live_state === 'uncertain' || storage === 'uncertain';
  const sameTransport = $('transport').value === (state.status.transport || 'usb');
  const canWrite = !state.busy && deviceAllowed && state.status.backup_exists && sameTransport;
  $('compile').disabled = state.busy || !state.token || !state.status.dll_available;
  $('compile').textContent = state.busy ? 'Working…' : 'Compile preview';
  $('add').disabled = state.busy || state.filters.length >= 10;
  $('import').disabled = state.busy;
  $('export').disabled = state.busy;
  $('transport').disabled = state.busy || !deviceAllowed || !untouched;
  $('read-device').disabled = state.busy || !deviceAllowed || !untouched;
  $('apply').disabled = !canWrite || !state.status.dll_available || uncertain || !current;
  $('save').disabled = !canWrite || !state.status.dll_available || uncertain || !current;
  $('restore').disabled = !canWrite || storage === 'uncertain';
  $('restore-saved').disabled = !canWrite || storage === 'unchanged';
  $('download-compiled').disabled = state.busy || !current;
  $('preview-state').textContent = current ? 'Compiled preview · Not a measurement' : state.compiled ? 'Filters changed · Compile again' : 'Compile to preview';
  $('response-plot').style.opacity = state.compiled && !current ? '.4' : '1';
  $('filter-count').textContent = `${state.filters.length} / 10`;
}
function changed() {
  $('preamp-result').textContent = '';
  $('manual-preamp').hidden = $('preamp-mode').value !== 'manual';
  $('preamp-value').required = $('preamp-mode').value === 'manual';
  $('preamp-value').disabled = $('preamp-mode').value !== 'manual';
  updateControls();
}
function renderFilters() {
  $('filter-rows').replaceChildren();
  state.filters.forEach((filter, index) => {
    const row = document.createElement('tr');
    row.className = filter.enabled ? '' : 'disabled-row';
    const onCell = document.createElement('td');
    const enabled = document.createElement('input'); enabled.type = 'checkbox'; enabled.checked = filter.enabled;
    enabled.setAttribute('aria-label', `Enable filter ${index + 1}`);
    enabled.addEventListener('change', () => { filter.enabled = enabled.checked; row.className = enabled.checked ? '' : 'disabled-row'; changed(); });
    onCell.append(enabled); row.append(onCell);
    const label = document.createElement('td'); label.className = 'filter-label'; label.textContent = String(index + 1).padStart(2, '0'); row.append(label);
    const typeCell = document.createElement('td'); const typeSelect = document.createElement('select');
    typeSelect.setAttribute('aria-label', `Filter type, filter ${index + 1}`);
    for (const [value, text] of Object.entries(filterTypes)) {
      const option = document.createElement('option'); option.value = value; option.textContent = text; typeSelect.append(option);
    }
    typeSelect.value = filter.type || 'peak';
    typeSelect.addEventListener('change', () => {
      filter.type = typeSelect.value;
      if (isPass(filter.type)) { filter.gain = 0; filter.q = shelfQ; }
      else if (isShelf(filter.type)) filter.q = shelfQ;
      renderFilters(); changed();
      $('filter-rows').children[index].querySelector('select').focus();
    });
    typeCell.append(typeSelect); row.append(typeCell);
    for (const [key, title, min, max] of [['frequency', 'Frequency in Hz', 20, 20000], ['gain', 'Gain in dB', -12, 12], ['q', 'Q factor', 0.1, 20]]) {
      const cell = document.createElement('td'); const input = document.createElement('input');
      Object.assign(input, {type: 'number', value: filter[key], min: String(min), max: String(max), step: 'any', required: true});
      input.setAttribute('aria-label', `${title}, filter ${index + 1}`);
      if (key === 'gain' && isPass(filter.type)) {
        input.disabled = true; input.title = 'Pass filters do not use a gain setting.';
      }
      if (key === 'q' && isShelf(filter.type)) {
        input.disabled = true; input.value = '0.7071'; input.title = 'Shelf slope is fixed at S = 1. Q is not adjustable for shelves.';
      }
      input.addEventListener('input', () => { filter[key] = input.value === '' ? null : Number(input.value); changed(); });
      cell.append(input);
      if (key === 'q' && isShelf(filter.type)) {
        const note = document.createElement('small'); note.className = 'fixed-value-note'; note.textContent = 'Fixed slope S = 1'; cell.append(note);
      }
      row.append(cell);
    }
    const removeCell = document.createElement('td'); const remove = document.createElement('button');
    remove.type = 'button'; remove.className = 'quiet remove'; remove.setAttribute('aria-label', `Remove filter ${index + 1}`);
    remove.disabled = state.filters.length === 1;
    remove.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.6"><path d="m6 6 12 12M6 18 18 6"/></svg>';
    remove.addEventListener('click', () => { state.filters.splice(index, 1); renderFilters(); changed(); });
    removeCell.append(remove); row.append(removeCell); $('filter-rows').append(row);
  });
  updateControls();
}
async function api(path, body) {
  const response = await fetch(path, {method: body === undefined ? 'GET' : 'POST', headers: body === undefined ? {} : {'Content-Type': 'application/json', 'X-Session-Token': state.token || ''}, ...(body === undefined ? {} : {body: JSON.stringify(body)})});
  let result;
  try { result = await response.json(); } catch { throw new Error(`Local server returned an unreadable response (${response.status}).`); }
  if (!response.ok || result.error) throw new Error(result.error || `Request failed (${response.status}).`);
  return result;
}
async function refreshStatus() {
  const result = await api('/api/status');
  state.status = result; state.token = result.token;
  const storage = result.storage_state || 'unchanged';
  if (!state.transportDirty || result.live_state !== 'unchanged' || storage !== 'unchanged') {
    $('transport').value = result.transport || 'usb';
    state.transportDirty = false;
  }
  $('session-recovery').hidden = !result.session_recovered;
  const live = result.live_enabled && result.dll_available;
  $('connection').textContent = live ? 'Device access enabled' : 'Offline editor';
  $('connection').classList.toggle('ready', !!live);
  const device = typeof result.device === 'string' ? result.device : result.device?.name;
  $('device-state').textContent = !result.live_enabled
    ? 'Device access is disabled. You can edit filters and compile a preview offline.'
    : storage === 'uncertain' ? 'The saved preset could not be verified. Use Restore saved profile to recover the original preset before making more changes.'
    : result.live_state === 'uncertain' ? 'The live EQ write was not confirmed. Use Restore live backup before applying again.'
    : storage === 'saved' ? 'Custom EQ is saved on the headset. Your original backup is retained. Restore saved profile restores both the saved preset and live EQ.'
    : result.live_state === 'applied' ? 'Temporary EQ is active. You can save it to the headset, or restore the live backup before reading another preset.'
    : !result.dll_available ? 'The Audeze coefficient library is unavailable. Compilation is disabled; reading and restoring remain available.'
    : `${device ? device + '. ' : 'Device access is enabled. '}${result.backup_exists ? 'A preset backup is available.' : 'Choose a custom preset in Audeze HQ, then read it to create a backup.'}`;
  updateControls();
}
function svgElement(name, attrs, text) {
  const node = document.createElementNS('http://www.w3.org/2000/svg', name);
  Object.entries(attrs).forEach(([key, val]) => node.setAttribute(key, String(val)));
  if (text !== undefined) node.textContent = text;
  return node;
}
function plot(curves = []) {
  const svg = $('response-plot'); svg.replaceChildren();
  const width = Math.max(270, svg.clientWidth), height = width < 600 ? 220 : 280;
  svg.setAttribute('viewBox', `0 0 ${width} ${height}`);
  const left = 40, right = width - 23, top = 20, bottom = height - 36;
  const points = curves.flatMap(c => c.points || []).filter(p => Number.isFinite(p[1]));
  const values = points.map(p => p[1]);
  const floor = -120;
  const clipped = values.some(db => db < floor);
  const yMin = Math.max(floor, Math.min(-12, Math.floor(Math.min(0, ...values) / 6) * 6));
  const yMax = Math.max(6, Math.ceil(Math.max(0, ...values) / 6) * 6);
  const x = f => left + Math.log10(f / 20) / 3 * (right - left);
  const y = db => top + (yMax - db) / (yMax - yMin) * (bottom - top);
  for (let db = yMin; db <= yMax; db += yMax - yMin > 60 ? 12 : 6) {
    svg.append(svgElement('line', {x1: left, x2: right, y1: y(db), y2: y(db), stroke: db === 0 ? '#a5b7c3' : '#e5ebef', 'stroke-width': 1}));
    svg.append(svgElement('text', {x: left - 12, y: y(db) + 4, 'text-anchor': 'end'}, `${db > 0 ? '+' : ''}${db}`));
  }
  for (const f of (width < 600 ? [20, 100, 1000, 10000, 20000] : [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000])) {
    svg.append(svgElement('line', {x1: x(f), x2: x(f), y1: top, y2: bottom, stroke: '#edf0f2', 'stroke-width': 1}));
    svg.append(svgElement('text', {x: x(f), y: height - 14, 'text-anchor': 'middle'}, f >= 1000 ? `${f / 1000}k` : f));
  }
  const defs = svgElement('defs', {}), clip = svgElement('clipPath', {id: 'plot-clip'});
  clip.append(svgElement('rect', {x: left, y: top, width: right - left, height: bottom - top})); defs.append(clip); svg.append(defs);
  curves.forEach((curve, index) => {
    const valid = (curve.points || []).filter(p => Number.isFinite(p[0]) && Number.isFinite(p[1]) && p[0] >= 20 && p[0] <= 20000);
    const d = valid.map((p, i) => `${i ? 'L' : 'M'}${x(p[0]).toFixed(2)},${y(p[1]).toFixed(2)}`).join(' ');
    svg.append(svgElement('path', {d, fill: 'none', stroke: curveColors[index % curveColors.length], 'stroke-width': 2.2, 'stroke-linejoin': 'round', 'clip-path': 'url(#plot-clip)', ...(index ? {'stroke-dasharray': '7 4'} : {})}));
  });
  $('plot-empty').hidden = curves.length > 0;
  $('plot-clipping').hidden = !clipped;
  svg.setAttribute('aria-label', curves.length ? `Calculated frequency response of ${curves.length} sample-rate banks, including preamp. Not measured on the headset.` : 'Frequency response plot. Compile filters to show the calculated response.');
  $('curve-legend').replaceChildren();
  curves.forEach((curve, index) => {
    const item = document.createElement('span'); item.className = 'legend-item';
    const line = document.createElement('span'); line.className = 'legend-line'; line.style.background = curveColors[index % curveColors.length];
    item.append(line, document.createTextNode(`${Number(curve.rate) / 1000} kHz`)); $('curve-legend').append(item);
  });
  if (!curves.length) $('curve-legend').textContent = '20 Hz–20 kHz · Logarithmic frequency';
}
async function action(work) {
  setBusy(true); $('message').hidden = true;
  try { await work(); } catch (error) { showMessage(error.message, true); await refreshStatus().catch(() => {}); }
  finally { setBusy(false); }
}
$('editor').addEventListener('submit', event => {
  event.preventDefault();
  action(async () => {
    const input = payload(); const result = await api('/api/compile', input);
    state.compiled = result; state.signature = JSON.stringify(input);
    plot(result.curves || []);
    $('preamp-result').textContent = `${Number(result.preamp).toFixed(2)} dB`;
    $('compile-summary').textContent = Number.isFinite(result.max_error_db) ? `Fit error ≤ ${result.max_error_db.toFixed(3)} dB` : 'Coefficients compiled';
    if (result.experimental_filters) $('compile-summary').textContent += ' · Experimental filters';
    $('details-content').textContent = JSON.stringify(result, null, 2);
    showMessage('Preview compiled. Nothing has been sent to the headset.');
  });
});
$('add').addEventListener('click', () => { if (state.filters.length < 10) { state.filters.push({type: 'peak', frequency: 1000, gain: 0, q: 1, enabled: true}); renderFilters(); changed(); } });
$('preamp-mode').addEventListener('change', changed);
$('preamp-value').addEventListener('input', changed);
$('export').addEventListener('click', () => { if ($('editor').reportValidity()) download('maxwell-peq.json', payload()); });
$('import').addEventListener('click', () => $('import-file').click());
$('import-file').addEventListener('change', async event => {
  const file = event.target.files[0]; if (!file) return;
  try {
    if (file.size > 100000) throw new Error('This file is too large. Import a filter JSON file under 100 KB.');
    const data = JSON.parse(await file.text());
    if (!data || typeof data !== 'object' || Array.isArray(data)) throw new Error('A preset must be a JSON object.');
    if (!Array.isArray(data.filters) || data.filters.length < 1 || data.filters.length > 10) throw new Error('The JSON must contain a filters array with 1 to 10 filters.');
    const filters = data.filters.map((f, i) => {
      if (!f || typeof f !== 'object') throw new Error(`Filter ${i + 1} must be an object.`);
      const type = f.type === undefined || f.type === 'PK' ? 'peak' : f.type;
      if (typeof type !== 'string' || !Object.hasOwn(filterTypes, type)) throw new Error(`Filter ${i + 1}: unknown filter type.`);
      const gain = f.gain === undefined && isPass(type) ? 0 : f.gain;
      const q = f.q === undefined && (isShelf(type) || isPass(type)) ? shelfQ : f.q;
      if (!Number.isFinite(f.frequency) || f.frequency < 20 || f.frequency > 20000 || !Number.isFinite(gain) || gain < -12 || gain > 12 || !Number.isFinite(q) || q < 0.1 || q > 20 || (f.enabled !== undefined && typeof f.enabled !== 'boolean')) throw new Error(`Filter ${i + 1} is invalid. Use 20–20000 Hz, −12 to +12 dB, and Q 0.1–20.`);
      if (isPass(type) && gain !== 0) throw new Error(`Filter ${i + 1}: pass filters require gain 0.`);
      if (isShelf(type) && Math.abs(q - shelfQ) > 0.00001) throw new Error(`Filter ${i + 1}: shelves use fixed slope S = 1. Omit Q or set it to 0.7071.`);
      return {type, frequency: f.frequency, gain, q: isShelf(type) ? shelfQ : q, enabled: f.enabled !== false};
    });
    const preamp = data.preamp === undefined ? 'auto' : data.preamp;
    if (preamp !== 'auto' && (!Number.isFinite(preamp) || preamp > 0 || preamp < -60)) throw new Error('Preamp must be "auto" or a number between −60 and 0 dB.');
    state.filters = filters; $('preamp-mode').value = preamp === 'auto' ? 'auto' : 'manual'; $('preamp-value').value = preamp === 'auto' ? 0 : preamp;
    renderFilters(); changed(); showMessage('Filters imported. Compile a preview to check them.');
  } catch (error) { showMessage(error.message, true); }
  event.target.value = '';
});
$('download-compiled').addEventListener('click', () => { if (isCurrent()) download('maxwell-peq-compiled.json', state.compiled); });
$('transport').addEventListener('change', () => {
  state.transportDirty = true;
  updateControls();
  showMessage('Read and back up the preset through this connection before applying or saving EQ.');
});
for (const [id, endpoint, message] of [
  ['read-device', '/api/read_device', 'Current preset read and backed up.'],
  ['apply', '/api/apply', 'Temporary EQ sent. Listen at low volume to check the result.'],
  ['save', '/api/save', 'Custom EQ saved, verified, and applied. The original backup is retained.'],
  ['restore', '/api/restore', 'Original backup applied to live EQ.'],
  ['restore-saved', '/api/restore_saved', 'Original saved preset and live EQ restored.']
]) {
  $(id).addEventListener('click', () => action(async () => {
    const sendsFilters = id === 'apply' || id === 'save';
    if (sendsFilters && (!$('editor').reportValidity() || !isCurrent())) throw new Error('Compile the current filters before applying or saving.');
    const result = await api(endpoint, sendsFilters ? payload() : id === 'read-device' ? {transport: $('transport').value} : {});
    if (id === 'read-device') state.transportDirty = false;
    $('details-content').textContent = JSON.stringify(result, null, 2);
    showMessage(result.message || (id === 'read-device' && result.backup_exists === false ? 'Preset read. Choose a custom preset in Audeze HQ, then read again to create a backup.' : message));
    await refreshStatus();
  }));
}
renderFilters(); plot();
new ResizeObserver(() => plot(state.compiled?.curves || [])).observe($('response-plot').parentElement);
refreshStatus().catch(error => { $('connection').textContent = 'Local server unavailable'; $('device-state').textContent = 'Start the local server and reload this page.'; showMessage(error.message, true); updateControls(); });
