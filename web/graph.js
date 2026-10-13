'use strict';
// Response graph with draggable filter handles. The instant curve uses the
// same RBJ formulas as extended_filters.py at 48 kHz; app.js swaps in the
// compiled, quantized 48 kHz bank once the server has checked it.
const PEQGraph = (() => {
  const SHELF_Q = Math.SQRT1_2;
  const RATE = 48000;
  const RANGE = 15;
  const FREQS = Array.from({length: 360}, (_, i) => 20 * 1000 ** (i / 359));
  const NS = 'http://www.w3.org/2000/svg';
  const isPass = type => type === 'low_pass' || type === 'high_pass';
  const isShelf = type => type === 'low_shelf' || type === 'high_shelf';
  const typeNames = {peak: 'peak', low_shelf: 'low shelf', high_shelf: 'high shelf', low_pass: 'low-pass', high_pass: 'high-pass'};

  function coefficients(f) {
    const w = 2 * Math.PI * f.frequency / RATE, cos = Math.cos(w), sin = Math.sin(w);
    const q = isShelf(f.type) ? SHELF_Q : f.q;
    const alpha = sin / (2 * q);
    let b0, b1, b2, a0, a1, a2;
    if (f.type === 'low_pass' || f.type === 'high_pass') {
      const sign = f.type === 'low_pass' ? 1 : -1;
      b0 = b2 = (1 - sign * cos) / 2; b1 = sign * (1 - sign * cos);
      a0 = 1 + alpha; a1 = -2 * cos; a2 = 1 - alpha;
    } else if (isShelf(f.type)) {
      const A = 10 ** (f.gain / 40), beta = 2 * Math.sqrt(A) * alpha, s = f.type === 'low_shelf' ? 1 : -1;
      b0 = A * ((A + 1) - s * (A - 1) * cos + beta);
      b1 = s * 2 * A * ((A - 1) - s * (A + 1) * cos);
      b2 = A * ((A + 1) - s * (A - 1) * cos - beta);
      a0 = (A + 1) + s * (A - 1) * cos + beta;
      a1 = -s * 2 * ((A - 1) + s * (A + 1) * cos);
      a2 = (A + 1) + s * (A - 1) * cos - beta;
    } else {
      const A = 10 ** (f.gain / 40);
      b0 = 1 + alpha * A; b1 = -2 * cos; b2 = 1 - alpha * A;
      a0 = 1 + alpha / A; a1 = -2 * cos; a2 = 1 - alpha / A;
    }
    return [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0];
  }

  function magnitude([b0, b1, b2, a1, a2], frequency) {
    const w = 2 * Math.PI * frequency / RATE, c1 = Math.cos(w), s1 = Math.sin(w), c2 = Math.cos(2 * w), s2 = Math.sin(2 * w);
    const nr = b0 + b1 * c1 + b2 * c2, ni = b1 * s1 + b2 * s2;
    const dr = 1 + a1 * c1 + a2 * c2, di = a1 * s1 + a2 * s2;
    return 10 * Math.log10(Math.max(1e-30, (nr * nr + ni * ni) / (dr * dr + di * di)));
  }

  const usable = f => f.enabled && [f.frequency, f.gain, f.q].every(Number.isFinite) && f.frequency >= 20 && f.frequency <= 20000 && f.q >= 0.1;

  function response(filters, freqs = FREQS) {
    const stages = filters.filter(usable).map(coefficients);
    return freqs.map(fr => [fr, stages.reduce((sum, c) => sum + magnitude(c, fr), 0)]);
  }

  function el(name, attrs = {}, text) {
    const node = document.createElementNS(NS, name);
    for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
    if (text !== undefined) node.textContent = text;
    return node;
  }

  const formatHz = f => f >= 1000 ? `${+(f / 1000).toFixed(f >= 10000 ? 1 : 2)} kHz` : `${Math.round(f)} Hz`;
  const signed = (v, digits = 1) => `${v > 0 ? '+' : v < 0 ? '−' : ''}${Math.abs(v).toFixed(digits)}`;

  function create(svg, handlers) {
    const box = {width: 800, height: 320, left: 40, right: 14, top: 14, bottom: 26};
    const layers = {grid: el('g', {class: 'g-grid'}), fill: el('path', {class: 'g-fill'}), single: el('path', {class: 'g-single'}),
      curve: el('path', {class: 'g-curve'}), handles: el('g', {class: 'g-handles'}), tip: el('g', {class: 'g-tip', visibility: 'hidden'})};
    const clipId = 'g-clip';
    const clipRect = el('rect');
    const defs = el('defs'); const clip = el('clipPath', {id: clipId}); clip.append(clipRect); defs.append(clip);
    const plot = el('g', {'clip-path': `url(#${clipId})`}); plot.append(layers.fill, layers.single, layers.curve);
    const tipBg = el('rect', {rx: 5}); const tipText = el('text'); layers.tip.append(tipBg, tipText);
    svg.append(defs, layers.grid, plot, layers.handles, layers.tip);
    let model = {filters: [], selected: -1, compiled: null, invalid: false};
    let drag = null, wheelTimer = null;
    const pool = [];

    const x = f => box.left + Math.log10(f / 20) / 3 * (box.width - box.left - box.right);
    const fx = px => 20 * 10 ** (3 * (px - box.left) / (box.width - box.left - box.right));
    const y = db => box.top + (RANGE - db) / (2 * RANGE) * (box.height - box.top - box.bottom);
    const gy = py => RANGE - (py - box.top) / (box.height - box.top - box.bottom) * 2 * RANGE;
    const roundHz = f => Math.min(20000, Math.max(20, f < 1000 ? Math.round(f) : Math.round(f / 10) * 10));
    const roundDb = g => Math.min(12, Math.max(-12, Math.round(g * 10) / 10));
    const handleDb = f => isPass(f.type) ? 0 : (Number.isFinite(f.gain) ? Math.max(-12, Math.min(12, f.gain)) : 0);

    function grid() {
      layers.grid.replaceChildren();
      const {left, right, top, bottom, width, height} = box;
      for (let db = -RANGE + 3; db < RANGE; db += 3) {
        layers.grid.append(el('line', {x1: left, x2: width - right, y1: y(db), y2: y(db), class: db === 0 ? 'zero' : db % 6 ? 'minor' : ''}));
        if (db % 6 === 0) layers.grid.append(el('text', {x: left - 8, y: y(db) + 4, 'text-anchor': 'end'}, db > 0 ? `+${db}` : db < 0 ? `−${-db}` : '0'));
      }
      const labels = new Set([20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]);
      for (const decade of [10, 100, 1000, 10000]) {
        for (let k = 1; k < 10; k++) {
          const f = decade * k; if (f < 20 || f > 20000) continue;
          layers.grid.append(el('line', {x1: x(f), x2: x(f), y1: top, y2: height - bottom, class: k === 1 ? '' : 'minor'}));
        }
      }
      layers.grid.append(el('line', {x1: x(20000), x2: x(20000), y1: top, y2: height - bottom}));
      for (const f of labels) {
        if (box.width < 640 && ![20, 100, 1000, 10000, 20000].includes(f)) continue;
        layers.grid.append(el('text', {x: x(f), y: height - 8, 'text-anchor': f === 20 ? 'start' : f === 20000 ? 'end' : 'middle'}, f >= 1000 ? `${f / 1000}k` : f));
      }
      layers.grid.append(el('text', {x: width - right - 2, y: top + 12, 'text-anchor': 'end', class: 'unit'}, 'dB'));
      clipRect.setAttribute('x', left); clipRect.setAttribute('y', top);
      clipRect.setAttribute('width', width - left - right); clipRect.setAttribute('height', height - top - bottom);
    }

    function path(points) {
      return points.map(([f, db], i) => `${i ? 'L' : 'M'}${x(f).toFixed(1)},${y(Math.max(-RANGE - 6, Math.min(RANGE + 6, db))).toFixed(1)}`).join('');
    }

    function draw() {
      const points = model.compiled || response(model.filters);
      const line = path(points);
      layers.curve.setAttribute('d', line);
      layers.curve.classList.toggle('invalid', !!model.invalid);
      layers.curve.classList.toggle('estimate', !model.compiled);
      layers.fill.setAttribute('d', points.length ? `${line}L${x(20000)},${y(0)}L${x(20)},${y(0)}Z` : '');
      const chosen = model.filters[model.selected];
      layers.single.setAttribute('d', chosen && usable(chosen) && model.filters.filter(usable).length > 1 ? path(response([chosen])) : '');
      while (pool.length < model.filters.length) pool.push(makeHandle(pool.length));
      pool.forEach((handle, index) => {
        const f = model.filters[index];
        if (!f) { handle.node.remove(); return; }
        if (!handle.node.isConnected) layers.handles.append(handle.node);
        const valid = Number.isFinite(f.frequency) && f.frequency >= 20 && f.frequency <= 20000;
        handle.node.setAttribute('transform', `translate(${x(valid ? f.frequency : 1000).toFixed(1)},${y(handleDb(f)).toFixed(1)})`);
        handle.node.setAttribute('class', `g-handle${index === model.selected ? ' selected' : ''}${f.enabled ? '' : ' off'}${isPass(f.type) ? ' pass' : ''}`);
        handle.node.setAttribute('aria-label', `Filter ${index + 1}, ${typeNames[f.type]}, ${valid ? formatHz(f.frequency) : 'invalid frequency'}${isPass(f.type) ? '' : `, ${signed(f.gain || 0)} dB`}${isShelf(f.type) ? '' : `, Q ${Number(f.q).toFixed(2)}`}${f.enabled ? '' : ', off'}. Arrow keys move it, plus and minus change Q.`);
      });
      tip();
    }

    function tip() {
      const index = drag ? drag.index : -1;
      const f = model.filters[index];
      if (!f) { layers.tip.setAttribute('visibility', 'hidden'); return; }
      const parts = [formatHz(f.frequency)];
      if (!isPass(f.type)) parts.push(`${signed(f.gain)} dB`);
      if (!isShelf(f.type)) parts.push(`Q ${f.q.toFixed(2)}`);
      tipText.textContent = parts.join('  ·  ');
      const width = tipText.getComputedTextLength() + 16;
      let tx = x(f.frequency) - width / 2, ty = y(handleDb(f)) - 42;
      tx = Math.max(box.left, Math.min(box.width - box.right - width, tx));
      if (ty < box.top) ty = y(handleDb(f)) + 18;
      tipBg.setAttribute('x', tx); tipBg.setAttribute('y', ty); tipBg.setAttribute('width', width); tipBg.setAttribute('height', 24);
      tipText.setAttribute('x', tx + 8); tipText.setAttribute('y', ty + 16);
      layers.tip.setAttribute('visibility', 'visible');
    }

    function makeHandle(index) {
      const node = el('g', {tabindex: 0, role: 'button', 'data-index': index});
      node.append(el('circle', {r: 15, class: 'hit'}), el('circle', {r: 11, class: 'dot'}), el('text', {y: 4, 'text-anchor': 'middle'}, index + 1));
      node.addEventListener('keydown', event => keyMove(index, event));
      node.addEventListener('keyup', event => { if (event.key.startsWith('Arrow') || event.key === '+' || event.key === '-' || event.key === '=') handlers.onCommit(index); });
      node.addEventListener('focus', () => handlers.onSelect(index, {scroll: false}));
      return {node};
    }

    function keyMove(index, event) {
      const f = model.filters[index]; if (!f) return;
      const fine = event.shiftKey;
      const change = {};
      if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
        const step = 2 ** ((event.key === 'ArrowRight' ? 1 : -1) / (fine ? 48 : 12));
        change.frequency = roundHz(f.frequency * step);
        if (change.frequency === f.frequency) change.frequency = Math.min(20000, Math.max(20, f.frequency + (event.key === 'ArrowRight' ? 1 : -1) * (f.frequency < 1000 ? 1 : 10)));
      } else if ((event.key === 'ArrowUp' || event.key === 'ArrowDown') && !isPass(f.type)) {
        change.gain = roundDb((f.gain || 0) + (event.key === 'ArrowUp' ? 1 : -1) * (fine ? 0.1 : 0.5));
      } else if ((event.key === '+' || event.key === '=' || event.key === '-') && !isShelf(f.type)) {
        change.q = Math.round(Math.min(20, Math.max(0.1, f.q * (event.key === '-' ? 1 / 1.1 : 1.1))) * 100) / 100;
      } else if (event.key === 'Delete') {
        event.preventDefault(); handlers.onRemove(index); return;
      } else return;
      event.preventDefault();
      handlers.onDrag(index, change);
    }

    function point(event) {
      const rect = svg.getBoundingClientRect();
      return [event.clientX - rect.left, event.clientY - rect.top];
    }

    svg.addEventListener('pointerdown', event => {
      const target = event.target.closest('[data-index]');
      if (!target || event.button !== 0) return;
      const index = Number(target.dataset.index);
      event.preventDefault();
      target.focus({preventScroll: true});
      handlers.onSelect(index, {scroll: true});
      svg.setPointerCapture(event.pointerId);
      const [px, py] = point(event);
      const f = model.filters[index];
      drag = {index, dx: px - x(f.frequency), dy: py - y(handleDb(f)), moved: false};
      svg.classList.add('dragging');
      tip();
    });
    svg.addEventListener('pointermove', event => {
      if (!drag) return;
      const [px, py] = point(event);
      const f = model.filters[drag.index];
      const change = {frequency: roundHz(fx(px - drag.dx))};
      if (!isPass(f.type)) change.gain = roundDb(gy(py - drag.dy));
      drag.moved = true;
      handlers.onDrag(drag.index, change);
    });
    const end = () => {
      if (!drag) return;
      const {index, moved} = drag; drag = null;
      svg.classList.remove('dragging');
      tip();
      if (moved) handlers.onCommit(index);
    };
    svg.addEventListener('pointerup', end);
    svg.addEventListener('pointercancel', end);
    svg.addEventListener('wheel', event => {
      const target = event.target.closest('[data-index]');
      const index = target ? Number(target.dataset.index) : model.selected;
      const f = model.filters[index];
      if (!f || isShelf(f.type) || !Number.isFinite(f.q)) return;
      event.preventDefault();
      const q = Math.round(Math.min(20, Math.max(0.1, f.q * Math.exp(-event.deltaY * 0.0015))) * 100) / 100;
      if (q === f.q) return;
      handlers.onDrag(index, {q});
      clearTimeout(wheelTimer);
      wheelTimer = setTimeout(() => handlers.onCommit(index), 350);
    }, {passive: false});
    svg.addEventListener('dblclick', event => {
      if (event.target.closest('[data-index]')) return;
      const [px, py] = point(event);
      if (px < box.left || px > box.width - box.right || py < box.top || py > box.height - box.bottom) return;
      handlers.onAdd(roundHz(fx(px)), roundDb(gy(py)));
    });

    new ResizeObserver(() => {
      const rect = svg.parentElement.getBoundingClientRect();
      box.width = Math.max(320, Math.round(rect.width)); box.height = Math.max(200, Math.round(rect.height));
      svg.setAttribute('viewBox', `0 0 ${box.width} ${box.height}`);
      svg.setAttribute('width', box.width); svg.setAttribute('height', box.height);
      grid(); draw();
    }).observe(svg.parentElement);

    return {
      render(next) { model = next; draw(); },
      focusHandle(index) { pool[index]?.node.focus({preventScroll: true}); },
    };
  }

  return {create, response, SHELF_Q, isPass, isShelf};
})();
