/* Shared helpers for the GUI-improvement mocks: DOM building, drag
   handling, keyboard routing to the last-pointed-at mock, the two slider
   styles (current egui look vs proposed), and a small photo "develop"
   model built from CSS filters and blend overlays. */
window.SP = (() => {
  const A = 'gui-improvements/';
  const IMG = {
    car: A + 'car.jpg',
    photo: A + 'photo.jpg',
    thumbs: Array.from({ length: 11 }, (_, i) => `${A}strip-${i + 1}.jpg`),
  };
  const demos = {};

  const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
  const lerp = (a, b, t) => a + (b - a) * t;
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const el = (html) => {
    const t = document.createElement('template');
    t.innerHTML = html.trim();
    return t.content.firstElementChild;
  };
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  /** Signed number with a real minus sign; zero has no sign. */
  const signed = (v, d = 0) => {
    const r = Number(v.toFixed(d));
    if (r === 0) return (0).toFixed(d);
    return (r > 0 ? '+' : '−') + Math.abs(r).toFixed(d);
  };
  const thin = (n) => Math.round(n).toLocaleString('en-US').replace(/,/g, ' ');
  const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  const cmdKey = (e) => (isMac ? e.metaKey : e.ctrlKey);

  /** Pointer drag with capture. `start` may return false to ignore. */
  function drag(node, { start, move, end }) {
    node.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      const ctx = start ? start(e) : {};
      if (ctx === false) return;
      e.preventDefault();
      try { node.setPointerCapture(e.pointerId); } catch (_) { /* synthetic events */ }
      const onMove = (ev) => move && move(ev, e, ctx);
      const onUp = (ev) => {
        node.removeEventListener('pointermove', onMove);
        node.removeEventListener('pointerup', onUp);
        node.removeEventListener('pointercancel', onUp);
        end && end(ev, e, ctx);
      };
      node.addEventListener('pointermove', onMove);
      node.addEventListener('pointerup', onUp);
      node.addEventListener('pointercancel', onUp);
    });
  }

  // ---------------------------------------------------------------- keys
  let active = null;
  function setActive(stage) {
    if (active === stage) return;
    if (active) active.closest('.pane')?.classList.remove('keys');
    active = stage;
    if (stage && stage._onKey) stage.closest('.pane')?.classList.add('keys');
  }
  function onKey(stage, fn) {
    stage._onKey = fn;
  }

  function boot() {
    $$('.stage[data-demo]').forEach((st) => {
      const fn = demos[st.dataset.demo];
      if (fn) {
        try {
          fn(st);
        } catch (err) {
          console.error(st.dataset.demo, err);
          st.append(el(`<div class="demo-error">Mock failed to start: ${esc(err.message)}</div>`));
        }
      }
      st.addEventListener('pointerenter', () => setActive(st));
      st.addEventListener('pointerdown', () => setActive(st));
      st.addEventListener('focusin', () => setActive(st));
    });
    document.addEventListener('keydown', (e) => {
      if (!active || !active._onKey) return;
      const t = e.target;
      if (t && t.matches && t.matches('input, textarea, select, [contenteditable="true"]') && !t.hasAttribute('data-passkeys')) return;
      const r = active.getBoundingClientRect();
      if (r.bottom < 0 || r.top > innerHeight) return;
      if (active._onKey(e) === true) e.preventDefault();
    });
  }

  // ---------------------------------------------------- egui-style slider
  /** Reproduces param_slider: label, trailing fill from the left end,
   *  18 px rounded handle, fixed 1-decimal value box, 1.8 px modified dot,
   *  double-click label or track to reset, drag the value box. */
  function egSlider({ label, min, max, value, def = 0, onChange }) {
    const row = el(`<div class="eg-row"><span class="eg-label">${esc(label)}<i class="eg-dot"></i></span><div class="eg-track"><div class="eg-fill"></div><div class="eg-handle"></div></div><span class="eg-value"></span></div>`);
    const track = $('.eg-track', row), fill = $('.eg-fill', row), handle = $('.eg-handle', row), val = $('.eg-value', row);
    let v = value;
    const pos = (x) => (x - min) / (max - min);
    function render() {
      const p = clamp(pos(v), 0, 1);
      const left = `calc(6.5px + ${p} * (100% - 13px))`;
      fill.style.width = left;
      handle.style.left = left;
      val.textContent = v.toFixed(1);
      row.classList.toggle('mod', Math.abs(v - def) > 0.0005);
    }
    function set(nv, silent) {
      v = clamp(nv, min, max);
      render();
      if (!silent && onChange) onChange(v);
    }
    const fromX = (x) => {
      const r = track.getBoundingClientRect();
      return min + clamp((x - r.left - 6.5) / (r.width - 13), 0, 1) * (max - min);
    };
    drag(track, { start: (e) => set(fromX(e.clientX)), move: (e) => set(fromX(e.clientX)) });
    drag(val, {
      start: () => ({ v0: v }),
      move: (e, s, c) => set(c.v0 + (e.clientX - s.clientX) * ((max - min) / 150)),
    });
    track.addEventListener('dblclick', () => set(def));
    $('.eg-label', row).addEventListener('dblclick', () => set(def));
    $('.eg-label', row).title = 'Double-click to reset';
    track.title = 'Double-click to reset';
    render();
    return { row, get: () => v, set };
  }

  // ------------------------------------------------------ proposed slider
  /** Fill from the neutral point, formatted values with units, accent when
   *  changed, hover reset, label scrubbing, typed entry, keyboard steps,
   *  optional gradient track and log scale. */
  function spSlider(o) {
    const { label, min, max, def = 0, fmt = (v) => signed(v, 0), decimals = 0, gradient, scale = 'linear', onChange } = o;
    let v = o.value ?? def;
    const bipolar = o.bipolar ?? (min < 0 && max > 0);
    const log = scale === 'log';
    const toPos = (x) => (log ? Math.log(x / min) / Math.log(max / min) : (x - min) / (max - min));
    const fromPos = (p) => (log ? min * Math.pow(max / min, p) : min + p * (max - min));
    const round = (x) => Number(x.toFixed(decimals));
    const step = o.step ?? Math.pow(10, -decimals) * (max - min > 50 ? 1 : 1);
    const row = el(`<div class="sl-row"><span class="sl-label" title="Drag to scrub · double-click to reset">${esc(label)}</span><div class="sl-track${gradient ? ' grad' : ''}"><div class="sl-rail"></div><div class="sl-zero"></div><div class="sl-fill"></div><div class="sl-thumb" tabindex="0" role="slider" aria-label="${esc(label)}"></div></div><input class="sl-value" spellcheck="false" aria-label="${esc(label)} value"><button class="sl-reset" title="Reset ${esc(label)}" aria-label="Reset ${esc(label)}">↺</button></div>`);
    const track = $('.sl-track', row), fill = $('.sl-fill', row), thumb = $('.sl-thumb', row), input = $('.sl-value', row), zero = $('.sl-zero', row), rail = $('.sl-rail', row);
    if (gradient) rail.style.background = gradient;
    const origin = bipolar ? toPos(0) : log ? 0 : toPos(min);
    const px = (p) => `calc(7px + ${p} * (100% - 14px))`;
    zero.style.left = px(gradient ? toPos(def) : origin);
    if (!bipolar && !gradient) zero.style.display = 'none';
    function render() {
      const p = clamp(toPos(v), 0, 1);
      thumb.style.left = px(p);
      const a = Math.min(p, origin), b = Math.max(p, origin);
      fill.style.left = px(a);
      fill.style.width = `calc(${b - a} * (100% - 14px))`;
      if (document.activeElement !== input) input.value = fmt(v);
      const mod = Math.abs(v - def) > Math.pow(10, -decimals) / 2;
      row.classList.toggle('mod', mod);
      thumb.setAttribute('aria-valuenow', v);
      thumb.setAttribute('aria-valuetext', fmt(v));
    }
    function set(nv, silent) {
      v = round(clamp(nv, min, max));
      render();
      if (!silent && onChange) onChange(v);
    }
    function reset() {
      row.classList.add('flash');
      set(def);
      setTimeout(() => row.classList.remove('flash'), 220);
    }
    const fromX = (x) => {
      const r = track.getBoundingClientRect();
      return fromPos(clamp((x - r.left - 7) / (r.width - 14), 0, 1));
    };
    drag(track, {
      start: (e) => {
        track.classList.add('dragging');
        set(fromX(e.clientX));
        thumb.focus({ preventScroll: true });
      },
      move: (e) => set(fromX(e.clientX)),
      end: () => track.classList.remove('dragging'),
    });
    track.addEventListener('dblclick', reset);
    const lab = $('.sl-label', row);
    drag(lab, {
      start: () => ({ p0: toPos(v), moved: false }),
      move: (e, s, c) => {
        const dx = e.clientX - s.clientX;
        if (Math.abs(dx) > 2) c.moved = true;
        const w = track.getBoundingClientRect().width - 14;
        set(fromPos(clamp(c.p0 + (dx / w) * (e.shiftKey ? 0.1 : 1), 0, 1)));
      },
    });
    lab.addEventListener('dblclick', reset);
    $('.sl-reset', row).addEventListener('click', reset);
    thumb.addEventListener('keydown', (e) => {
      const mul = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
      let d = 0;
      if (e.key === 'ArrowRight' || e.key === 'ArrowUp') d = 1;
      else if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') d = -1;
      else if (e.key === 'Home') { set(min); e.preventDefault(); return; }
      else if (e.key === 'End') { set(max); e.preventDefault(); return; }
      else return;
      e.preventDefault();
      e.stopPropagation();
      if (log) set(fromPos(clamp(toPos(v) + d * 0.004 * mul, 0, 1)));
      else set(v + d * step * mul);
    });
    input.addEventListener('focus', () => {
      input.value = String(round(v));
      requestAnimationFrame(() => input.select());
    });
    input.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') input.blur();
      if (e.key === 'Escape') { input.value = String(round(v)); input.blur(); }
      if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
        e.preventDefault();
        set(v + (e.key === 'ArrowUp' ? 1 : -1) * step * (e.shiftKey ? 10 : 1));
        input.value = String(round(v));
      }
    });
    input.addEventListener('blur', () => {
      const n = parseFloat(input.value.replace('−', '-').replace(/[^\d.+-]/g, ''));
      if (Number.isFinite(n)) set(n);
      else render();
      input.value = fmt(v);
    });
    render();
    return { row, get: () => v, set, reset };
  }

  // ------------------------------------------------------- photo "develop"
  /** A photo with CSS-filter tone/colour, WB and tint overlays, and a
   *  vignette. Good enough to make every mock react visibly. */
  function photo(src, { fit = 'cover', ratio } = {}) {
    const wrap = el(`<div class="ph"><img alt="" draggable="false"><div class="ph-wb"></div><div class="ph-tint"></div><div class="ph-vig"></div></div>`);
    const img = $('img', wrap);
    img.src = src;
    img.style.objectFit = fit;
    if (ratio) wrap.style.aspectRatio = ratio;
    const wb = $('.ph-wb', wrap), tint = $('.ph-tint', wrap), vig = $('.ph-vig', wrap);
    function apply(p = {}) {
      img.style.filter = filterFor(p);
      const temp = p.temp ?? 5500;
      const mired = 1e6 / 5500 - 1e6 / temp; // >0 warmer
      const a = clamp(Math.abs(mired) / 160, 0, 0.55);
      wb.style.background = mired >= 0 ? `rgba(255,145,40,${a})` : `rgba(60,130,255,${a})`;
      const t = (p.tint ?? 0) / 150;
      tint.style.background = t >= 0 ? `rgba(230,60,200,${Math.abs(t) * 0.45})` : `rgba(60,200,90,${Math.abs(t) * 0.45})`;
      const vg = (p.vignette ?? 0) / 100;
      vig.style.opacity = Math.abs(vg);
      vig.style.background = vg < 0
        ? 'radial-gradient(ellipse at center, transparent 45%, rgba(0,0,0,.85) 100%)'
        : 'radial-gradient(ellipse at center, transparent 45%, rgba(255,255,255,.8) 100%)';
    }
    apply({});
    return { el: wrap, img, apply };
  }
  function filterFor(p = {}) {
    const ev = p.exposure ?? 0;
    const bright = Math.pow(2, ev * 0.55) * (1 + (p.shadows ?? 0) * 0.0012 + (p.highlights ?? 0) * 0.0008 + (p.brightness ?? 0) * 0.002);
    const contrast = 1 + (p.contrast ?? 0) / 220 + (p.clarity ?? 0) / 400 + (p.blacks ?? 0) * -0.0012 + (p.whites ?? 0) * 0.0012;
    const sat = Math.max(0, 1 + (p.saturation ?? 0) / 100 + (p.vibrance ?? 0) / 180);
    let f = `brightness(${bright.toFixed(3)}) contrast(${contrast.toFixed(3)}) saturate(${sat.toFixed(3)})`;
    if (p.sepia) f += ` sepia(${p.sepia})`;
    if (p.hue) f += ` hue-rotate(${p.hue}deg)`;
    if (p.gray) f += ` grayscale(${p.gray})`;
    if (p.extra) f += ` ${p.extra}`;
    return f;
  }

  /** Relative time like "2 min ago". */
  function ago(sec) {
    if (sec < 45) return 'just now';
    if (sec < 90) return '1 min ago';
    if (sec < 3600) return `${Math.round(sec / 60)} min ago`;
    return `${Math.round(sec / 3600)} h ago`;
  }

  return { A, IMG, demos, clamp, lerp, $, $$, el, esc, signed, thin, drag, onKey, boot, egSlider, spSlider, photo, filterFor, ago, isMac, cmdKey };
})();
