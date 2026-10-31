/* Viewport mocks: 1 compare, 2 zoom toolbar + navigator, 3 tool banner, 4 crop. */
(() => {
  const { IMG, demos, clamp, $, $$, el, drag, onKey, thin } = SP;

  // A flat "original" rendition versus the developed "edit" of the same photo.
  const ORIG = 'brightness(.9) contrast(.8) saturate(.72) sepia(.08)';
  const EDIT = 'contrast(1.05) saturate(1.1)';

  // Global SVG filters for clipping overlays (sRGB thresholds).
  const defs = document.getElementById('svg-defs');
  const ns = 'http://www.w3.org/2000/svg';
  function svgEl(tag, attrs, parent) {
    const n = document.createElementNS(ns, tag);
    for (const k in attrs) n.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(n);
    return n;
  }
  const steps = (n, hiOnly) => Array.from({ length: n }, (_, i) => (hiOnly ? (i === n - 1 ? 1 : 0) : i === 0 ? 1 : 0)).join(' ');
  /** Builds a clipping-overlay filter. With `tone` it first applies a
   *  tone table so the overlay tracks edits (used by the histogram mock). */
  SP.clipFilter = function clipFilter(id, kind, withTone) {
    const f = svgEl('filter', { id, 'color-interpolation-filters': 'sRGB', x: 0, y: 0, width: 1, height: 1 }, defs);
    let toneFns = [];
    if (withTone) {
      const ct = svgEl('feComponentTransfer', {}, f);
      toneFns = ['R', 'G', 'B'].map((c) => svgEl('feFunc' + c, { type: 'table', tableValues: '0 1' }, ct));
    }
    if (kind === 'hi') {
      const ct = svgEl('feComponentTransfer', {}, f);
      ['R', 'G', 'B'].forEach((c) => svgEl('feFunc' + c, { type: 'discrete', tableValues: steps(40, true) }, ct));
      svgEl('feColorMatrix', { type: 'matrix', values: '0 0 0 0 1  0 0 0 0 0.16  0 0 0 0 0.12  1 1 1 0 0' }, f);
    } else {
      svgEl('feColorMatrix', { type: 'matrix', values: '.2126 .7152 .0722 0 0  .2126 .7152 .0722 0 0  .2126 .7152 .0722 0 0  0 0 0 1 0' }, f);
      const ct = svgEl('feComponentTransfer', {}, f);
      ['R', 'G', 'B'].forEach((c) => svgEl('feFunc' + c, { type: 'discrete', tableValues: steps(30, false) }, ct));
      svgEl('feColorMatrix', { type: 'matrix', values: '0 0 0 0 0.25  0 0 0 0 0.55  0 0 0 0 1  1 0 0 0 0' }, f);
    }
    return toneFns;
  };
  SP.toneFilter = function toneFilter(id) {
    const f = svgEl('filter', { id, 'color-interpolation-filters': 'sRGB', x: 0, y: 0, width: 1, height: 1 }, defs);
    const ct = svgEl('feComponentTransfer', {}, f);
    return ['R', 'G', 'B'].map((c) => svgEl('feFunc' + c, { type: 'table', tableValues: '0 1' }, ct));
  };
  SP.clipFilter('clip-hi', 'hi');
  SP.clipFilter('clip-lo', 'lo');

  function topbar(extraRight = '', extraLeft = '') {
    return `<div class="topbar mini">
      <span class="brand">Shop<span>Photo</span></span>
      <span class="seg"><span>Library</span><span class="on">Develop</span></span>
      ${extraLeft}
      <span class="spacer"></span>
      ${extraRight}
    </div>`;
  }

  // ------------------------------------------------------------ 1 compare
  demos['compare-before'] = (st) => {
    st.innerHTML = `<div class="mockwin">
      ${topbar(`<span class="beforetag" hidden>BEFORE&nbsp;&nbsp;(\\)</span><span class="badge">SDR</span><span class="btn pill small">View</span><span class="btn pill small">Fit</span><span class="btn pill small primary">Export</span>`)}
      <div class="viewport vp-300"><img class="fitimg" src="${IMG.car}" alt=""><span class="zoomtag">Fit</span></div>
    </div>
    <div class="sim"><b>Simulate</b><button class="btn small" data-k>Press \\</button><span>The inspector and filmstrip are omitted.</span></div>`;
    const img = $('.fitimg', st), tag = $('.beforetag', st);
    let before = false;
    const render = () => {
      img.style.filter = before ? ORIG : EDIT;
      tag.hidden = !before;
    };
    const toggle = () => { before = !before; render(); };
    $('[data-k]', st).onclick = toggle;
    onKey(st, (e) => { if (e.key === '\\') { toggle(); return true; } });
    render();
  };

  demos['compare-after'] = (st) => {
    st.innerHTML = `<div class="mockwin">
      ${topbar(`<span class="badge">SDR</span><span class="btn pill small primary">Export</span>`)}
      <div class="viewport vp-300 cmp" data-mode="split">
        <div class="cmp-single"><img src="${IMG.car}" alt=""><span class="cmp-flag">Before</span></div>
        <div class="cmp-split"><img class="a" src="${IMG.car}" alt=""><img class="b" src="${IMG.car}" alt=""><div class="cmp-div"><span class="cmp-grip">‹ ›</span></div><span class="cmp-l">Before</span><span class="cmp-r">After</span></div>
        <div class="cmp-sbs"><div><img src="${IMG.car}" alt=""><span class="cmp-l">Before</span></div><div><img src="${IMG.car}" alt=""><span class="cmp-l">After</span></div></div>
        <div class="vp-hud"><span class="seg small" role="tablist"><span data-m="toggle">Toggle</span><span data-m="split">Split</span><span data-m="sbs">Side by side</span></span><span class="kbd">Y</span><span class="hud-sep"></span><span class="hud-note">Before = <b>Original</b> ▾</span></div>
      </div>
    </div>`;
    const vp = $('.cmp', st);
    const single = $('.cmp-single img', st), flag = $('.cmp-flag', st);
    const [sa, sb] = $$('.cmp-split img', st);
    const [x1, x2] = $$('.cmp-sbs img', st);
    sa.style.filter = EDIT; sb.style.filter = ORIG; x1.style.filter = ORIG; x2.style.filter = EDIT;
    const div = $('.cmp-div', st);
    let mode = 'split', split = 0.5, showBefore = false;
    function render() {
      vp.dataset.mode = mode;
      $$('[data-m]', st).forEach((n) => n.classList.toggle('on', n.dataset.m === mode));
      single.style.filter = showBefore ? ORIG : EDIT;
      flag.hidden = !showBefore;
      sb.style.clipPath = `inset(0 ${(1 - split) * 100}% 0 0)`;
      div.style.left = split * 100 + '%';
    }
    $$('[data-m]', st).forEach((n) => (n.onclick = () => { mode = n.dataset.m; render(); }));
    drag($('.cmp-split', st), {
      start: (e) => {
        const r = vp.getBoundingClientRect();
        split = clamp((e.clientX - r.left) / r.width, 0.03, 0.97);
        render();
      },
      move: (e) => {
        const r = vp.getBoundingClientRect();
        split = clamp((e.clientX - r.left) / r.width, 0.03, 0.97);
        render();
      },
    });
    onKey(st, (e) => {
      if (e.key === '\\') { mode = 'toggle'; showBefore = !showBefore; render(); return true; }
      if (e.key === 'y' || e.key === 'Y') {
        const order = ['toggle', 'split', 'sbs'];
        mode = order[(order.indexOf(mode) + 1) % 3];
        render();
        return true;
      }
    });
    render();
  };

  // --------------------------------------------------------- 2 zoom / nav
  function zoomModel(vp, imgs, { onChange } = {}) {
    const NW = 1752, NH = 1296; // nominal "100%" size of the demo photo
    const m = { z: null, cx: 0.5, cy: 0.5 };
    const size = () => [vp.clientWidth, vp.clientHeight];
    const fitZ = () => { const [W, H] = size(); return Math.min(W / NW, H / NH); };
    const fillZ = () => { const [W, H] = size(); return Math.max(W / NW, H / NH); };
    m.eff = () => m.z ?? fitZ();
    m.fitZ = fitZ; m.fillZ = fillZ;
    m.clampCenter = () => {
      const [W, H] = size();
      const z = m.eff(), dw = NW * z, dh = NH * z;
      m.cx = dw > W ? clamp(m.cx, W / 2 / dw, 1 - W / 2 / dw) : 0.5;
      m.cy = dh > H ? clamp(m.cy, H / 2 / dh, 1 - H / 2 / dh) : 0.5;
    };
    m.visible = () => {
      const [W, H] = size();
      const z = m.eff(), dw = NW * z, dh = NH * z;
      const w = Math.min(1, W / dw), h = Math.min(1, H / dh);
      return { x: m.cx - w / 2, y: m.cy - h / 2, w, h };
    };
    m.render = () => {
      m.clampCenter();
      const [W, H] = size();
      const z = m.eff(), dw = NW * z, dh = NH * z;
      const left = W / 2 - m.cx * dw, top = H / 2 - m.cy * dh;
      imgs.forEach((im) => Object.assign(im.style, { width: dw + 'px', height: dh + 'px', left: left + 'px', top: top + 'px' }));
      onChange && onChange();
    };
    m.pan = (dx, dy) => {
      const z = m.eff();
      m.cx -= dx / (NW * z);
      m.cy -= dy / (NH * z);
      m.render();
    };
    m.zoomAt = (z, px, py) => {
      const [W, H] = size();
      const z0 = m.eff();
      if (px != null) {
        const ix = m.cx + (px - W / 2) / (NW * z0), iy = m.cy + (py - H / 2) / (NH * z0);
        m.cx = ix; m.cy = iy;
      }
      m.z = z;
      m.render();
    };
    new ResizeObserver(() => m.render()).observe(vp);
    return m;
  }

  demos['zoom-before'] = (st) => {
    st.innerHTML = `<div class="mockwin">
      ${topbar(`<span class="badge">SDR</span><span class="menuwrap"><span class="btn pill small" data-view>View</span></span><span class="btn pill small" data-fit>Fit</span><span class="btn pill small primary">Export</span>`)}
      <div class="viewport vp-300 zoomvp"><img class="zimg" src="${IMG.car}" alt=""><img class="zimg ov hi" src="${IMG.car}" alt="" hidden><img class="zimg ov lo" src="${IMG.car}" alt="" hidden><span class="zoomtag"></span></div>
    </div>`;
    const vp = $('.zoomvp', st), imgs = $$('.zimg', st), tag = $('.zoomtag', st), fitBtn = $('[data-fit]', st);
    const hi = $('.ov.hi', st), lo = $('.ov.lo', st);
    hi.style.filter = 'url(#clip-hi)'; lo.style.filter = 'url(#clip-lo)';
    const m = zoomModel(vp, imgs, {
      onChange: () => {
        const t = m.z == null ? 'Fit' : Math.round(m.z * 100) + '%';
        tag.textContent = t; fitBtn.textContent = t;
      },
    });
    let clip = 0;
    const renderClip = () => { lo.hidden = !(clip & 1); hi.hidden = !(clip & 2); };
    const toggle = () => { m.z == null ? m.zoomAt(1) : ((m.z = null), m.render()); };
    fitBtn.onclick = toggle;
    vp.addEventListener('dblclick', (e) => {
      const r = vp.getBoundingClientRect();
      m.z == null ? m.zoomAt(1, e.clientX - r.left, e.clientY - r.top) : ((m.z = null), m.render());
    });
    drag(vp, { start: (e) => ({ x: e.clientX, y: e.clientY }), move: (e, s, c) => { if (m.z == null) return; m.pan(e.clientX - c.x, e.clientY - c.y); c.x = e.clientX; c.y = e.clientY; } });
    // View menu reproduces the real menu's bottom rows.
    const wrap = $('.menuwrap', st);
    $('[data-view]', st).onclick = () => {
      const open = $('.menu', wrap);
      if (open) { open.remove(); return; }
      const menu = el(`<div class="menu right">
        <div class="section">Soft proof</div><div class="mi on">Display native</div><div class="mi">sRGB</div><div class="mi">Display P3</div><div class="mi">Rec.2020</div><div class="mi">Import display ICC…</div><hr>
        <label class="mi"><span><input type="checkbox"> Gamut warning</span></label><hr>
        <div class="section">Accessibility</div><div class="mi faint">UI scale ─────●──── 100%</div><div class="mi faint">English · Nederlands</div>
        <label class="mi"><span><input type="checkbox" data-c="1"> Shadow clipping  ·  J</span></label>
        <label class="mi"><span><input type="checkbox" data-c="2"> Highlight clipping  ·  O</span></label></div>`);
      $$('[data-c]', menu).forEach((cb) => {
        cb.checked = !!(clip & +cb.dataset.c);
        cb.onchange = () => { clip ^= +cb.dataset.c; renderClip(); };
      });
      wrap.append(menu);
    };
    onKey(st, (e) => {
      if (e.metaKey || e.ctrlKey) return;
      if (e.key === 'j' || e.key === 'J') { clip ^= 1; renderClip(); return true; }
      if (e.key === 'o' || e.key === 'O') { clip ^= 2; renderClip(); return true; }
      if (e.key === 'z' || e.key === 'Z') { toggle(); return true; }
    });
    m.render();
  };

  demos['zoom-after'] = (st) => {
    st.innerHTML = `<div class="mockwin">
      ${topbar(`<span class="badge">SDR</span><span class="btn pill small primary">Export</span>`)}
      <div class="viewport vp-300 zoomvp"><img class="zimg" src="${IMG.car}" alt=""><img class="zimg ov hi" src="${IMG.car}" alt="" hidden><img class="zimg ov lo" src="${IMG.car}" alt="" hidden>
        <div class="nav" hidden><img src="${IMG.car}" alt=""><div class="nav-rect"></div><span class="nav-z"></span></div>
        <div class="vp-hud">
          <span class="seg small"><span data-z="fit">Fit</span><span data-z="fill">Fill</span><span data-z="0.5">50%</span><span data-z="1">100%</span><span data-z="2">200%</span></span>
          <span class="hud-sep"></span>
          <button class="hud-btn" data-c="1" title="Shadow clipping (J)"><span class="tri lo"></span></button>
          <button class="hud-btn" data-c="2" title="Highlight clipping (O)"><span class="tri hi"></span></button>
          <span class="hud-sep"></span>
          <button class="hud-btn wide" data-b title="Before / after (\\)">◐ Before</button>
        </div>
      </div>
    </div>`;
    const vp = $('.zoomvp', st), imgs = $$('.zimg', st), nav = $('.nav', st), rect = $('.nav-rect', st), navz = $('.nav-z', st);
    const hi = $('.ov.hi', st), lo = $('.ov.lo', st), main = imgs[0];
    hi.style.filter = 'url(#clip-hi)'; lo.style.filter = 'url(#clip-lo)';
    let clip = 0, before = false;
    const m = zoomModel(vp, imgs, {
      onChange: () => {
        const z = m.eff();
        const key = m.z == null ? 'fit' : Math.abs(m.z - m.fillZ()) < 1e-6 ? 'fill' : String(m.z);
        $$('[data-z]', st).forEach((n) => n.classList.toggle('on', n.dataset.z === key));
        const zoomed = m.eff() > m.fitZ() + 1e-6;
        nav.hidden = !zoomed;
        const v = m.visible();
        Object.assign(rect.style, { left: v.x * 100 + '%', top: v.y * 100 + '%', width: v.w * 100 + '%', height: v.h * 100 + '%' });
        navz.textContent = Math.round(z * 100) + '%';
      },
    });
    const render = () => {
      lo.hidden = !(clip & 1); hi.hidden = !(clip & 2);
      $$('[data-c]', st).forEach((b) => b.classList.toggle('on', !!(clip & +b.dataset.c)));
      $('[data-b]', st).classList.toggle('on', before);
      main.style.filter = before ? ORIG : '';
    };
    $$('[data-z]', st).forEach((n) => (n.onclick = () => {
      const k = n.dataset.z;
      if (k === 'fit') { m.z = null; m.render(); } else m.zoomAt(k === 'fill' ? m.fillZ() : +k);
    }));
    $$('[data-c]', st).forEach((b) => (b.onclick = () => { clip ^= +b.dataset.c; render(); }));
    $('[data-b]', st).onclick = () => { before = !before; render(); };
    drag(vp, {
      start: (e) => (e.target.closest('.vp-hud, .nav') ? false : { x: e.clientX, y: e.clientY }),
      move: (e, s, c) => { m.pan(e.clientX - c.x, e.clientY - c.y); c.x = e.clientX; c.y = e.clientY; },
    });
    vp.addEventListener('dblclick', (e) => {
      if (e.target.closest('.vp-hud, .nav')) return;
      const r = vp.getBoundingClientRect();
      m.z == null ? m.zoomAt(1, e.clientX - r.left, e.clientY - r.top) : ((m.z = null), m.render());
    });
    const navTo = (e) => {
      const r = nav.getBoundingClientRect();
      m.cx = clamp((e.clientX - r.left) / r.width, 0, 1);
      m.cy = clamp((e.clientY - r.top) / r.height, 0, 1);
      m.render();
    };
    drag(nav, { start: navTo, move: navTo });
    onKey(st, (e) => {
      if (e.metaKey || e.ctrlKey) return;
      const k = e.key.toLowerCase();
      if (k === 'j') { clip ^= 1; render(); return true; }
      if (k === 'o') { clip ^= 2; render(); return true; }
      if (k === '\\') { before = !before; render(); return true; }
      if (k === 'z') { m.z == null ? m.zoomAt(1) : ((m.z = null), m.render()); return true; }
    });
    m.render();
    render();
  };

  // ------------------------------------------------------- 3 tool banner
  const TOOLS = {
    color: { name: 'Color range', icon: '◎', old: 'Click the color to include · Esc cancels', hint: 'Click the colour to include', oneShot: true },
    curve: { name: 'Curve pipette', icon: '⌇', old: 'Click a tone and drag vertically · Esc cancels', hint: 'Click a tone, then drag up or down', oneShot: true },
    heal: { name: 'Heal', icon: '✚', old: 'Draw in the image · Esc finishes', hint: 'Click a spot or drag a stroke · source is chosen for you' },
    upright: { name: 'Guided upright', icon: '⟂', old: 'Drag along an edge that should be straight · Esc finishes', hint: 'Drag along 2–4 edges that should be straight' },
    subject: { name: 'Select subject', icon: '✦', old: 'Click the object or sky to select · Esc cancels', hint: 'Click the object or sky to select it', oneShot: true },
  };
  function toolDemo(st, proposed) {
    st.innerHTML = `<div class="mockwin">
      <div class="viewport vp-300 toolvp"><img class="fitimg cover top" src="${IMG.photo}" alt=""><div class="marks"></div>
        ${proposed ? '<div class="banner" hidden></div>' : '<span class="oldhint" hidden></span>'}
      </div>
      <div class="toolrow">${Object.entries(TOOLS).map(([k, t]) => `<button class="btn small" data-t="${k}">${t.name}</button>`).join('')}</div>
    </div>`;
    const vp = $('.toolvp', st), marks = $('.marks', st);
    let tool = null, lineStart = null;
    const hintEl = proposed ? $('.banner', st) : $('.oldhint', st);
    function render(msg) {
      $$('[data-t]', st).forEach((b) => b.classList.toggle('on', b.dataset.t === tool));
      vp.classList.toggle('crosshair', !!tool);
      if (!tool) { hintEl.hidden = true; return; }
      const t = TOOLS[tool];
      hintEl.hidden = false;
      if (!proposed) { hintEl.textContent = t.old; return; }
      hintEl.innerHTML = `<span class="b-ico">${t.icon}</span><span class="b-title">${t.name}</span><span class="b-hint">${msg || t.hint}</span><button class="b-btn" data-x><span class="kbd">Esc</span> Cancel</button><button class="b-btn primary" data-d><span class="kbd">↵</span> Done</button>`;
      $('[data-x]', hintEl).onclick = () => finish(true);
      $('[data-d]', hintEl).onclick = () => finish(false);
    }
    function finish(cancel) {
      if (cancel && proposed) $$('.m-new', marks).forEach((n) => n.remove());
      $$('.m-new', marks).forEach((n) => n.classList.remove('m-new'));
      tool = null;
      render();
    }
    $$('[data-t]', st).forEach((b) => (b.onclick = () => { tool = tool === b.dataset.t ? null : b.dataset.t; render(); }));
    const local = (e) => { const r = vp.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };
    drag(vp, {
      start: (e) => {
        if (!tool || e.target.closest('.banner')) return false;
        const [x, y] = local(e);
        if (tool === 'upright') { lineStart = [x, y]; const ln = el('<div class="m-line m-new"></div>'); marks.append(ln); return { ln }; }
        return {};
      },
      move: (e, s, c) => {
        if (tool !== 'upright' || !c.ln) return;
        const [x, y] = local(e);
        const dx = x - lineStart[0], dy = y - lineStart[1];
        Object.assign(c.ln.style, { left: lineStart[0] + 'px', top: lineStart[1] + 'px', width: Math.hypot(dx, dy) + 'px', transform: `rotate(${Math.atan2(dy, dx)}rad)` });
      },
      end: (e, s, c) => {
        if (!tool) return;
        const [x, y] = local(e);
        if (tool === 'upright') return;
        if (tool === 'heal') {
          marks.append(el(`<div class="m-spot m-new" style="left:${x}px;top:${y}px"></div>`));
          marks.append(el(`<div class="m-spot src m-new" style="left:${x - 34}px;top:${y - 18}px"></div>`));
          return;
        }
        const ring = el(`<div class="m-ring" style="left:${x}px;top:${y}px"></div>`);
        marks.append(ring);
        setTimeout(() => ring.remove(), 900);
        if (proposed) {
          const done = { color: 'Sampled · Oklab a +0.012 b −0.041', curve: 'Point added at 72% · drag to adjust', subject: 'Subject found · mask created as Local 3' }[tool];
          render(`<span class="ok">✓</span> ${done}`);
          setTimeout(() => { if (tool && TOOLS[tool].oneShot) finish(false); }, 1100);
        } else {
          tool = null; // silently ends, as the app does
          render();
        }
      },
    });
    onKey(st, (e) => {
      if (!tool) return;
      if (e.key === 'Escape') { finish(true); return true; }
      if (e.key === 'Enter' && proposed) { finish(false); return true; }
    });
    render();
  }
  demos['toolhud-before'] = (st) => toolDemo(st, false);
  demos['toolhud-after'] = (st) => toolDemo(st, true);

  // --------------------------------------------------------------- 4 crop
  const SRC_W = 11656, SRC_H = 8621;
  function cropDemo(st, proposed) {
    st.innerHTML = `<div class="mockwin">
      <div class="viewport vp-320 cropvp">
        <div class="crop-img"><img src="${IMG.car}" alt=""><div class="crop-box"><div class="grid3"></div>${proposed ? '<i class="c tl"></i><i class="c tr"></i><i class="c bl"></i><i class="c br"></i><i class="e t"></i><i class="e b"></i><i class="e l"></i><i class="e r"></i><span class="dims"></span>' : '<i class="d tl"></i><i class="d tr"></i><i class="d bl"></i><i class="d br"></i>'}</div></div>
        <div class="crop-done" hidden><img src="${IMG.car}" alt=""></div>
        ${proposed ? '<div class="banner"></div>' : '<span class="oldcrop">Crop — drag corners or rectangle · toggle Crop in Geometry to finish</span>'}
      </div>
      <div class="toolrow">${proposed
        ? '<span class="seg small aspects"><span data-a="free">Free</span><span data-a="orig">Original</span><span data-a="1">1:1</span><span data-a="0.8">4:5</span><span data-a="1.5">3:2</span><span data-a="1.7778">16:9</span></span><button class="btn small" data-x title="Swap orientation (X)">⇄</button>'
        : '<span class="sel on" data-crop>Crop</span><button class="btn small" data-reset>Reset crop</button><span class="menuwrap"><button class="btn small" data-aspect>Aspect ▸</button></span>'}</div>
    </div>`;
    const vp = $('.cropvp', st), holder = $('.crop-img', st), box = $('.crop-box', st), done = $('.crop-done', st);
    const IA = SRC_W / SRC_H;
    let crop = [0.08, 0.1, 0.92, 0.9], saved = crop.slice(), aspect = null, cropping = true;
    function layout() {
      const W = vp.clientWidth - 24, H = vp.clientHeight - (proposed ? 70 : 50);
      const s = Math.min(W / IA, H);
      holder.style.width = s * IA + 'px';
      holder.style.height = s + 'px';
    }
    function render() {
      const [x0, y0, x1, y1] = crop;
      Object.assign(box.style, { left: x0 * 100 + '%', top: y0 * 100 + '%', width: (x1 - x0) * 100 + '%', height: (y1 - y0) * 100 + '%' });
      if (proposed) {
        const w = Math.round((x1 - x0) * SRC_W), h = Math.round((y1 - y0) * SRC_H);
        $('.dims', st).textContent = `${thin(w)} × ${thin(h)} px · ${((w * h) / 1e6).toFixed(1)} MP`;
        $$('[data-a]', st).forEach((n) => n.classList.toggle('on', n.dataset.a === (aspect == null ? 'free' : aspect.key)));
        $('.banner', st).hidden = !cropping;
      }
      holder.hidden = !cropping;
      done.hidden = cropping;
      if (!cropping) {
        const img = $('img', done);
        const cw = x1 - x0, ch = y1 - y0;
        const W = vp.clientWidth - 24, H = vp.clientHeight - 24;
        const s = Math.min(W / (cw * IA), H / ch);
        Object.assign(done.style, { width: cw * IA * s + 'px', height: ch * s + 'px' });
        Object.assign(img.style, { width: IA * s + 'px', height: s + 'px', left: -x0 * IA * s + 'px', top: -y0 * s + 'px' });
      }
      const old = $('.oldcrop', st);
      if (old) old.hidden = !cropping;
    }
    // Fit a ratio (width/height in pixels) around the current centre.
    function fitAspect(r) {
      const cx = (crop[0] + crop[2]) / 2, cy = (crop[1] + crop[3]) / 2;
      let w = crop[2] - crop[0], h = (w * IA) / r;
      if (h > 1) { h = 1; w = (h * r) / IA; }
      if (w > 1) { w = 1; h = (w * IA) / r; }
      const x0 = clamp(cx - w / 2, 0, 1 - w), y0 = clamp(cy - h / 2, 0, 1 - h);
      crop = [x0, y0, x0 + w, y0 + h];
    }
    // The app's set_crop_aspect: maximal centred crop of that ratio.
    function appAspect(r) {
      if (IA > r) { const w = r / IA; crop = [(1 - w) / 2, 0, (1 + w) / 2, 1]; }
      else { const h = IA / r; crop = [0, (1 - h) / 2, 1, (1 + h) / 2]; }
    }
    const MIN = 0.05;
    const norm = (e) => { const r = holder.getBoundingClientRect(); return [(e.clientX - r.left) / r.width, (e.clientY - r.top) / r.height, r]; };
    drag(holder, {
      start: (e) => {
        if (!cropping) return false;
        const [u, v, r] = norm(e);
        const pts = { tl: [crop[0], crop[1]], tr: [crop[2], crop[1]], bl: [crop[0], crop[3]], br: [crop[2], crop[3]] };
        let handle = null, best = proposed ? 16 : 14;
        for (const k in pts) {
          const d = Math.hypot((pts[k][0] - u) * r.width, (pts[k][1] - v) * r.height);
          if (d < best) { best = d; handle = k; }
        }
        if (!handle && proposed) {
          const inY = v > crop[1] && v < crop[3], inX = u > crop[0] && u < crop[2];
          const near = (a, b) => Math.abs(a - b) * (b === crop[1] || b === crop[3] ? r.height : r.width) < 10;
          if (inX && near(v, crop[1])) handle = 't';
          else if (inX && near(v, crop[3])) handle = 'b';
          else if (inY && near(u, crop[0])) handle = 'l';
          else if (inY && near(u, crop[2])) handle = 'r';
        }
        box.classList.add('dragging');
        return { handle, u, v, c0: crop.slice() };
      },
      move: (e, s, c) => {
        const [u, v] = norm(e);
        const du = u - c.u, dv = v - c.v;
        let [x0, y0, x1, y1] = c.c0;
        const h = c.handle;
        if (!h) {
          const w = x1 - x0, hh = y1 - y0;
          x0 = clamp(x0 + du, 0, 1 - w); y0 = clamp(y0 + dv, 0, 1 - hh);
          crop = [x0, y0, x0 + w, y0 + hh];
        } else if (!proposed || !aspect) {
          if (h.includes('l')) x0 = clamp(x0 + du, 0, x1 - MIN);
          if (h.includes('r')) x1 = clamp(x1 + du, x0 + MIN, 1);
          if (h.includes('t')) y0 = clamp(y0 + dv, 0, y1 - MIN);
          if (h.includes('b')) y1 = clamp(y1 + dv, y0 + MIN, 1);
          crop = [x0, y0, x1, y1];
        } else {
          const r = aspect.r;
          if (h.length === 2) {
            // Corner: width follows the pointer, height follows the ratio;
            // the opposite corner stays anchored.
            const ax = h.includes('l') ? x1 : x0, ay = h.includes('t') ? y1 : y0;
            let w = Math.abs((h.includes('l') ? x0 + du : x1 + du) - ax);
            w = Math.max(w, MIN);
            let hh = (w * IA) / r;
            const maxW = h.includes('l') ? ax : 1 - ax, maxH = h.includes('t') ? ay : 1 - ay;
            if (w > maxW) { w = maxW; hh = (w * IA) / r; }
            if (hh > maxH) { hh = maxH; w = (hh * r) / IA; }
            x0 = h.includes('l') ? ax - w : ax; x1 = x0 + w;
            y0 = h.includes('t') ? ay - hh : ay; y1 = y0 + hh;
            crop = [x0, y0, x1, y1];
          } else {
            // Edge: that dimension follows the pointer, the other keeps
            // the ratio around its centre.
            const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
            if (h === 'l' || h === 'r') {
              if (h === 'l') x0 = clamp(x0 + du, 0, x1 - MIN); else x1 = clamp(x1 + du, x0 + MIN, 1);
              let hh = ((x1 - x0) * IA) / r;
              hh = Math.min(hh, 1);
              const w = (hh * r) / IA;
              if (h === 'l') x0 = x1 - w; else x1 = x0 + w;
              y0 = clamp(cy - hh / 2, 0, 1 - hh); y1 = y0 + hh;
            } else {
              if (h === 't') y0 = clamp(y0 + dv, 0, y1 - MIN); else y1 = clamp(y1 + dv, y0 + MIN, 1);
              let w = ((y1 - y0) * r) / IA;
              w = Math.min(w, 1);
              const hh = (w * IA) / r;
              if (h === 't') y0 = y1 - hh; else y1 = y0 + hh;
              x0 = clamp(cx - w / 2, 0, 1 - w); x1 = x0 + w;
            }
            crop = [x0, y0, x1, y1];
          }
        }
        render();
      },
      end: () => box.classList.remove('dragging'),
    });
    if (proposed) {
      $('.banner', st).innerHTML = `<span class="b-ico">⌗</span><span class="b-title">Crop</span><span class="b-hint">Drag handles · <span class="kbd">X</span> swaps ratio</span><button class="b-btn" data-cx><span class="kbd">Esc</span> Cancel</button><button class="b-btn primary" data-cd><span class="kbd">↵</span> Done</button>`;
      $('[data-cx]', st).onclick = () => { crop = saved.slice(); cropping = false; render(); showEdit(); };
      $('[data-cd]', st).onclick = () => { cropping = false; render(); showEdit(); };
      $$('[data-a]', st).forEach((n) => (n.onclick = () => {
        const k = n.dataset.a;
        if (k === 'free') aspect = null;
        else { const r = k === 'orig' ? IA : +k; aspect = { key: k, r }; fitAspect(r); }
        render();
      }));
      $('[data-x]', st).onclick = () => swap();
    } else {
      $('[data-crop]', st).onclick = () => { cropping = !cropping; $('[data-crop]', st).classList.toggle('on', cropping); render(); };
      $('[data-reset]', st).onclick = () => { crop = [0, 0, 1, 1]; render(); };
      const wrap = $('.menuwrap', st);
      $('[data-aspect]', st).onclick = () => {
        const open = $('.menu', wrap);
        if (open) { open.remove(); return; }
        const menu = el(`<div class="menu up"><div class="mi" data-r="orig">Original</div><div class="mi" data-r="1">1 : 1</div><div class="mi" data-r="0.8">4 : 5</div><div class="mi" data-r="1.5">3 : 2</div><div class="mi" data-r="1.7778">16 : 9</div></div>`);
        $$('[data-r]', menu).forEach((mi) => (mi.onclick = () => {
          if (mi.dataset.r === 'orig') crop = [0, 0, 1, 1]; else appAspect(+mi.dataset.r);
          menu.remove(); render();
        }));
        wrap.append(menu);
      };
    }
    function swap() {
      if (!aspect || aspect.r === 1) return;
      aspect = { key: aspect.key, r: 1 / aspect.r };
      fitAspect(aspect.r);
      render();
    }
    // "Edit crop" returns to the tool (proposed mock only).
    function showEdit() {
      if (!proposed) return;
      let b = $('.recrop', st);
      if (!b) {
        b = el('<button class="btn small recrop">Edit crop <span class="kbd">R</span></button>');
        $('.toolrow', st).append(b);
        b.onclick = () => { saved = crop.slice(); cropping = true; b.remove(); render(); };
      }
    }
    onKey(st, (e) => {
      if (!proposed) return;
      if (cropping && e.key === 'Enter') { cropping = false; render(); showEdit(); return true; }
      if (cropping && e.key === 'Escape') { crop = saved.slice(); cropping = false; render(); showEdit(); return true; }
      if (cropping && (e.key === 'x' || e.key === 'X')) { swap(); return true; }
      if (!cropping && (e.key === 'r' || e.key === 'R')) { $('.recrop', st)?.click(); return true; }
    });
    new ResizeObserver(() => { layout(); render(); }).observe(vp);
    layout();
    render();
  }
  demos['crop-before'] = (st) => cropDemo(st, false);
  demos['crop-after'] = (st) => cropDemo(st, true);
})();
