/* Inspector mocks: 5 sliders, 6 temperature scale, 7 card state,
   8 navigation, 9 histogram, 10 colour mixer/wheels, 11 presets, 12 history. */
(() => {
  const { IMG, demos, clamp, $, $$, el, esc, drag, onKey, signed, thin, egSlider, spSlider, photo, filterFor, ago } = SP;

  const TEMP_GRAD = 'linear-gradient(90deg, #4b7bff, #9cc0ff 30%, #e9e3d4 34%, #ffc46b 60%, #ff8a2a)';
  const TINT_GRAD = 'linear-gradient(90deg, #3fbf5a, #b7d9b9 45%, #d8cdd8 55%, #e05ad0)';

  // ------------------------------------------------------------ 5 sliders
  const BASIC = [
    { k: 'temp', label: 'Temp (K)', min: 2000, max: 50000, def: 5500, v: 5500 },
    { k: 'tint', label: 'Tint', min: -150, max: 150, def: 0, v: 0 },
    { k: 'exposure', label: 'Exposure', min: -5, max: 5, def: 0, v: 1.3 },
    { k: 'brightness', label: 'Brightness', min: -100, max: 100, def: 0, v: 0 },
    { k: 'contrast', label: 'Contrast', min: -100, max: 100, def: 0, v: 0 },
    { k: 'highlights', label: 'Highlights', min: -100, max: 100, def: 0, v: -13.5 },
    { k: 'shadows', label: 'Shadows', min: -100, max: 100, def: 0, v: 0 },
    { k: 'whites', label: 'Whites', min: -100, max: 100, def: 0, v: -8.6 },
    { k: 'blacks', label: 'Blacks', min: -100, max: 100, def: 0, v: -24 },
  ];
  const FMT = {
    temp: { fmt: (v) => `${thin(v)} K`, decimals: 0, step: 50, scale: 'log', gradient: TEMP_GRAD },
    tint: { fmt: (v) => signed(v, 0), decimals: 0, gradient: TINT_GRAD },
    exposure: { fmt: (v) => `${signed(v, 2)} EV`, decimals: 2, step: 0.05 },
  };
  function sliderPane(st, proposed) {
    const ph = photo(IMG.car);
    ph.el.classList.add('ph-strip');
    const card = el(`<div class="card col tight"><div class="section">${proposed ? '▾ Basic' : '▼ BASIC'}</div></div>`);
    const params = {};
    BASIC.forEach((p) => {
      params[p.k] = p.v;
      const onChange = (v) => { params[p.k] = v; ph.apply(params); };
      const s = proposed
        ? spSlider({ label: p.label.replace(' (K)', ''), min: p.min, max: p.max, def: p.def, value: p.v, decimals: 0, ...(FMT[p.k] || {}), onChange })
        : egSlider({ label: p.label, min: p.min, max: p.max, def: p.def, value: p.v, onChange });
      card.append(s.row);
    });
    st.append(ph.el, card);
    ph.apply(params);
  }
  demos['sliders-before'] = (st) => sliderPane(st, false);
  demos['sliders-after'] = (st) => sliderPane(st, true);

  // ------------------------------------------------------ 6 temperature
  const TICKS = [2500, 3200, 4000, 5000, 6500, 8000, 10000, 15000, 25000, 50000];
  function tempPane(st, proposed) {
    const ph = photo(IMG.car);
    ph.el.classList.add('ph-strip');
    const target = 6500;
    const status = el('<div class="tempstat"></div>');
    const ruler = el('<div class="ruler"></div>');
    const log = proposed;
    const min = 2000, max = 50000;
    const toPos = (k) => (log ? Math.log(k / min) / Math.log(max / min) : (k - min) / (max - min));
    let s;
    const update = (v) => {
      ph.apply({ temp: v });
      const tw = $('.eg-track, .sl-track', s.row).getBoundingClientRect().width - (proposed ? 14 : 13);
      const kpp = log ? (v * Math.log(max / min)) / tw : (max - min) / tw;
      const hit = Math.abs(v - target) <= 25;
      status.innerHTML = `<span>${proposed ? thin(v) + ' K' : v.toFixed(1)}</span><span class="faint">one pixel of travel here ≈ <b>${Math.round(kpp)} K</b></span><span class="target ${hit ? 'hit' : ''}">${hit ? '✓ 6 500 K reached' : 'target 6 500 K'}</span>`;
    };
    s = proposed
      ? spSlider({ label: 'Temp', min, max, def: 5500, value: 5500, decimals: 0, step: 50, scale: 'log', gradient: TEMP_GRAD, fmt: (v) => `${thin(v)} K`, onChange: update })
      : egSlider({ label: 'Temp (K)', min, max, def: 5500, value: 5500, onChange: update });
    const card = el('<div class="card col tight tempcard"></div>');
    const rw = el(`<div class="${proposed ? 'sl-row' : 'eg-row'} ruler-row"><span></span></div>`);
    rw.append(ruler);
    let last = -1e9;
    TICKS.forEach((k) => {
      const p = toPos(k);
      const inset = proposed ? 7 : 6.5;
      const t = el(`<i class="tick" style="left:calc(${inset}px + ${p} * (100% - ${inset * 2}px))"></i>`);
      ruler.append(t);
      // Label only when there is room (~34 px at a 150 px track).
      if (p - last > 0.12 || k === 50000) {
        const lab = el(`<span class="tlab" style="left:calc(${inset}px + ${p} * (100% - ${inset * 2}px))">${k >= 10000 ? k / 1000 + 'k' : thin(k)}</span>`);
        ruler.append(lab);
        last = p;
      }
    });
    // The daylight band that covers almost every photo.
    const band = el(`<i class="band" style="left:calc(${proposed ? 7 : 6.5}px + ${toPos(2500)} * (100% - ${proposed ? 14 : 13}px)); width:calc(${toPos(10000) - toPos(2500)} * (100% - ${proposed ? 14 : 13}px))"></i>`);
    ruler.prepend(band);
    card.append(s.row, rw, status);
    st.append(ph.el, card, el(`<div class="app tiny faint">Card shown at the real inspector width (356 px panel). Shaded band: 2 500–10 000 K, ${Math.round((toPos(10000) - toPos(2500)) * 100)}% of the track.</div>`));
    requestAnimationFrame(() => update(5500));
    new ResizeObserver(() => update(s.get())).observe(card);
  }
  demos['temp-before'] = (st) => tempPane(st, false);
  demos['temp-after'] = (st) => tempPane(st, true);

  // ---------------------------------------------------------- 7 card state
  const GROUPS = [
    { id: 'basic', title: 'Basic', rows: [
      { k: 'exposure', label: 'Exposure', min: -5, max: 5, v: 0.6, fmt: (v) => `${signed(v, 2)} EV`, decimals: 2, step: 0.05 },
      { k: 'contrast', label: 'Contrast', min: -100, max: 100, v: 12 },
      { k: 'highlights', label: 'Highlights', min: -100, max: 100, v: -20 },
    ] },
    { id: 'curve', title: 'Tone Curve', rows: [
      { k: 'lights', label: 'Lights', min: -100, max: 100, v: 0 },
      { k: 'darks', label: 'Darks', min: -100, max: 100, v: 0 },
    ] },
    { id: 'color', title: 'Color', rows: [
      { k: 'vibrance', label: 'Vibrance', min: -100, max: 100, v: 18 },
      { k: 'saturation', label: 'Saturation', min: -100, max: 100, v: 0 },
    ] },
    { id: 'detail', title: 'Detail', rows: [
      { k: 'sharp', label: 'Sharpness', min: 0, max: 100, v: 40, fmt: (v) => String(Math.round(v)) },
      { k: 'nr', label: 'Luminance', min: 0, max: 100, v: 0, fmt: (v) => String(Math.round(v)) },
    ] },
    { id: 'effects', title: 'Effects', rows: [
      { k: 'clarity', label: 'Clarity', min: -100, max: 100, v: 0 },
      { k: 'vignette', label: 'Vignette', min: -100, max: 100, v: -35 },
    ] },
  ];
  function cardsPane(st, proposed) {
    const ph = photo(IMG.car);
    ph.el.classList.add('ph-strip');
    const vals = {}, bypass = {}, sliders = {};
    GROUPS.forEach((g) => g.rows.forEach((r) => (vals[r.k] = r.v)));
    const insp = el(`<div class="insp">${proposed ? '<div class="insp-head"><span class="section">Develop</span><span class="spacer"></span><label class="solo"><input type="checkbox"> Solo</label></div>' : ''}</div>`);
    const open = { basic: true, curve: false, color: false, detail: false, effects: false };
    const apply = () => {
      const p = {};
      GROUPS.forEach((g) => {
        if (bypass[g.id]) return;
        g.rows.forEach((r) => (p[r.k] = vals[r.k]));
      });
      ph.apply({ exposure: p.exposure, contrast: (p.contrast ?? 0) + (p.lights ?? 0) * 0.3 - (p.darks ?? 0) * 0.3, highlights: p.highlights, vibrance: p.vibrance, saturation: p.saturation, clarity: (p.clarity ?? 0) + (p.sharp ?? 0) * 0.3, vignette: p.vignette });
      GROUPS.forEach((g) => {
        const n = g.rows.filter((r) => Math.abs(vals[r.k]) > 0.0005).length;
        const card = $(`[data-g="${g.id}"]`, insp);
        card.classList.toggle('has-mod', n > 0);
        const b = $('.count', card);
        if (b) b.textContent = n;
      });
    };
    GROUPS.forEach((g) => {
      const card = el(`<div class="card gcard" data-g="${g.id}">
        <div class="ghead">${proposed
          ? `<span class="chev">▶</span><span class="section">${g.title}</span><span class="count"></span><span class="spacer"></span><button class="gbtn greset" title="Reset ${g.title}">↺</button><button class="gbtn geye" title="Bypass ${g.title} to compare">◉</button>`
          : `<span class="chev old">▶</span><span class="section">${g.title.toUpperCase()}</span>`}</div>
        <div class="gbody col tight"></div></div>`);
      g.rows.forEach((r) => {
        const onChange = (v) => { vals[r.k] = v; apply(); };
        const s = proposed
          ? spSlider({ label: r.label, min: r.min, max: r.max, def: 0, value: r.v, decimals: r.decimals ?? 0, step: r.step, fmt: r.fmt ?? ((v) => signed(v, 0)), onChange })
          : egSlider({ label: r.label, min: r.min, max: r.max, def: 0, value: r.v, onChange });
        sliders[r.k] = s;
        $('.gbody', card).append(s.row);
      });
      const setOpen = (o) => { open[g.id] = o; card.classList.toggle('open', o); };
      $('.ghead', card).addEventListener('click', (e) => {
        if (e.target.closest('.gbtn')) return;
        const next = !open[g.id];
        const solo = proposed && ($('.solo input', insp).checked || e.altKey);
        if (solo && next) GROUPS.forEach((o) => { if (o.id !== g.id) { open[o.id] = false; $(`[data-g="${o.id}"]`, insp).classList.remove('open'); } });
        setOpen(next);
      });
      if (proposed) {
        $('.greset', card).onclick = () => { g.rows.forEach((r) => sliders[r.k].reset()); };
        $('.geye', card).onclick = () => {
          bypass[g.id] = !bypass[g.id];
          card.classList.toggle('bypassed', !!bypass[g.id]);
          $('.geye', card).textContent = bypass[g.id] ? '◌' : '◉';
          apply();
        };
      }
      setOpen(open[g.id]);
      insp.append(card);
    });
    if (!proposed) {
      const reset = el('<div class="center"><button class="btn small">Reset all edits</button></div>');
      reset.querySelector('button').onclick = () => GROUPS.forEach((g) => g.rows.forEach((r) => sliders[r.k].set(0)));
      insp.append(reset);
    } else {
      $('.solo input', insp).onchange = (e) => {
        if (!e.target.checked) return;
        const first = GROUPS.find((g) => open[g.id]);
        GROUPS.forEach((g) => { if (g !== first) { open[g.id] = false; $(`[data-g="${g.id}"]`, insp).classList.remove('open'); } });
      };
    }
    st.append(ph.el, insp);
    apply();
  }
  demos['cards-before'] = (st) => cardsPane(st, false);
  demos['cards-after'] = (st) => cardsPane(st, true);

  // ------------------------------------------------------------ 8 navigation
  const S = (label, min, max, v, def = 0) => ({ t: 's', label, min, max, v, def });
  const C = (label, on) => ({ t: 'c', label, on });
  const B = (label) => ({ t: 'b', label });
  const X = (label, value) => ({ t: 'x', label, value });
  const T = (text) => ({ t: 't', text });
  const ADV = (row) => ({ ...row, adv: true });
  const NAV_CARDS = [
    { title: 'Basic', sec: 'Light', open: true, rows: [X('Profile', 'ShopPhoto Standard'), B('Auto balance'), X('Look', 'Standard'), S('Temp (K)', 2000, 50000, 5500, 5500), S('Tint', -150, 150, 0), S('Exposure', -5, 5, 0), S('Contrast', -100, 100, 0), S('Highlights', -100, 100, 0), S('Shadows', -100, 100, 0), S('Whites', -100, 100, 0), S('Blacks', -100, 100, 0), ADV(C('Recover clipped highlights', true)), ADV(S('Recovery', 0, 100, 100, 100))] },
    { title: 'Tone Curve', sec: 'Light', rows: [T('Curve editor (Y · R · G · B)'), S('Highlights', -100, 100, 0), S('Lights', -100, 100, 0), S('Darks', -100, 100, 0), S('Shadows', -100, 100, 0)] },
    { title: 'Color', sec: 'Color', rows: [S('Vibrance', -100, 100, 0), S('Saturation', -100, 100, 0), T('HSL · 8 bands'), ADV(T('Color grading · 3 wheels')), ADV(C('Monochrome', false))] },
    { title: 'Local & retouch', sec: 'Color', rows: [B('Brush · Radial · Gradient'), B('Heal · Clone')] },
    { title: 'Detail', sec: 'Detail', rows: [C('High-detail demosaic (AHD)', true), S('Sharpness', 0, 100, 0), ADV(S('Amount', 0, 150, 0)), ADV(S('Radius', 0.5, 3, 1, 1)), ADV(S('Detail', 0, 100, 25, 25)), ADV(S('Masking', 0, 100, 25, 25)), ADV(S('Halo guard', 0, 100, 50, 50)), X('AI denoise', 'Cleanest'), S('Luminance', 0, 100, 0), S('Chroma', 0, 100, 0)] },
    { title: 'Lens Corrections', sec: 'Optics', open: true, rows: [
      C('Enable lens corrections', true), X('Profile', 'Auto (XCD 4/45P)'), S('Vignetting', 0, 100, 100, 100), C('Remove color fringes', true), S('Purple fringe', 0, 100, 0),
      ADV(B('Auto Defringe')), ADV(X('Lens', 'XCD 4/45P')), ADV(T('Applied, interpolated to f/8.0')), ADV(B('Auto lateral CA')), ADV(S('Distortion', -0.5, 0.5, 0)), ADV(S('Red CA', -0.01, 0.01, 0)), ADV(S('Blue CA', -0.01, 0.01, 0)), ADV(C('Crop to valid pixels', true)), ADV(B('Save geometry to .splens')),
      ADV(B('Pick purple…  300° ±35°')), ADV(S('Purple hue', 0, 360, 300, 300)), ADV(S('Purple range', 2, 90, 35, 35)), ADV(S('Green fringe', 0, 100, 0)), ADV(B('Pick green…  120° ±35°')), ADV(S('Green hue', 0, 360, 120, 120)), ADV(S('Green range', 2, 90, 35, 35)), ADV(C('Wide / clipped highlight bloom', false)), ADV(S('Fringe width', 1, 256, 2, 2)), ADV(S('Edge protection', 0, 100, 55, 55))] },
    { title: 'Effects', sec: 'Detail', rows: [S('Texture', -100, 100, 0), S('Clarity', -100, 100, 0), S('Dehaze', -100, 100, 0), S('Vignette', -100, 100, 0), ADV(S('Grain', 0, 100, 0))] },
    { title: 'Geometry', sec: 'Geometry', rows: [B('Crop · Reset crop · Auto level'), S('Straighten', -45, 45, 0), S('Vertical', -100, 100, 0), S('Horizontal', -100, 100, 0), ADV(S('Scale', 10, 400, 100, 100)), ADV(B('Guided upright')), ADV(B('Distortion calibration'))] },
    { title: 'Resolution', sec: 'Output', rows: [T('Full — 11656 × 8742 · 101.9 MP'), T('2×2 supersample — 5828 × 4371')] },
    { title: 'HDR', sec: 'Output', rows: [C('HDR preview & export', false), S('Peak headroom', 0, 4, 2, 2), ADV(S('Ref. white', -2, 2, -0.4, -0.4)), ADV(S('Shoulder', 0, 100, 50, 50))] },
    { title: 'Metadata', sec: 'Output', rows: [T('x1d-ii-xcd45p-01.fff'), T('Camera  Hasselblad X1D II 50C'), T('ISO 100 · 1/320 s · f/8')] },
  ];
  const SECS = ['Light', 'Color', 'Detail', 'Optics', 'Geometry', 'Output'];
  function rowEl(r, proposed) {
    if (r.t === 's') return (proposed ? spSlider({ label: r.label, min: r.min, max: r.max, def: r.def, value: r.v, decimals: r.max - r.min < 5 ? 2 : 0, fmt: r.min >= 0 ? (v) => v.toFixed(r.max - r.min < 5 ? 2 : 0) : undefined }) : egSlider({ label: r.label, min: r.min, max: r.max, def: r.def, value: r.v })).row;
    if (r.t === 'c') return el(`<label class="chk"><input type="checkbox" ${r.on ? 'checked' : ''}> ${esc(r.label)}</label>`);
    if (r.t === 'b') return el(`<div><button class="btn small">${esc(r.label)}</button></div>`);
    if (r.t === 'x') return el(`<div class="row"><span class="xlab muted">${esc(r.label)}</span><span class="combo">${esc(r.value)}</span></div>`);
    return el(`<div class="muted tiny">${esc(r.text)}</div>`);
  }
  function rowText(r) { return (r.label || r.text || '') + ' ' + (r.value || ''); }

  demos['nav-before'] = (st) => {
    const sc = el(`<div class="insp scroll h440"><img class="histimg" src="${SP.A}current-histogram.jpg" alt="Histogram"><div class="plainhead">▸ Snapshots &amp; history</div></div>`);
    NAV_CARDS.forEach((c) => {
      const card = el(`<div class="card gcard ${c.open ? 'open' : ''}"><div class="ghead"><span class="chev old">▶</span><span class="section">${c.title.toUpperCase()}</span></div><div class="gbody col tight"></div></div>`);
      c.rows.forEach((r) => $('.gbody', card).append(rowEl(r, false)));
      $('.ghead', card).onclick = () => card.classList.toggle('open');
      if (c.title === 'Metadata') sc.append(el('<div class="center"><button class="btn small">Reset all edits</button></div>'));
      sc.append(card);
    });
    const count = NAV_CARDS[5].rows.length;
    st.append(sc, el(`<div class="tiny faint app">Lens Corrections alone: ${count} rows when expanded. The histogram scrolls away.</div>`));
  };

  demos['nav-after'] = (st) => {
    const wrap = el(`<div class="insp scroll h440 navafter">
      <div class="sticky">
        <img class="histimg small" src="${SP.A}current-histogram.jpg" alt="Histogram">
        <div class="row"><input class="field search" placeholder="Find a tool…  ⌘F" data-passkeys><span class="scount faint tiny"></span></div>
        <div class="jump">${SECS.map((s) => `<span data-s="${s}">${s}</span>`).join('')}</div>
      </div>
      <div class="navbody"></div></div>`);
    const body = $('.navbody', wrap);
    const cards = [];
    SECS.forEach((sec) => {
      const secEl = el(`<div class="navsec" data-sec="${sec}"><div class="secname">${sec}</div></div>`);
      NAV_CARDS.filter((c) => c.sec === sec).forEach((c) => {
        const essentials = c.rows.filter((r) => !r.adv), adv = c.rows.filter((r) => r.adv);
        const card = el(`<div class="card gcard ${c.open ? 'open' : ''}"><div class="ghead"><span class="chev">▶</span><span class="section">${c.title}</span></div><div class="gbody col tight"></div></div>`);
        const gb = $('.gbody', card);
        const rowsEls = [];
        essentials.forEach((r) => { const n = rowEl(r, true); rowsEls.push([r, n]); gb.append(n); });
        let advBox = null;
        if (adv.length) {
          const disc = el(`<button class="adv">Advanced · ${adv.length} more <span>▾</span></button>`);
          advBox = el('<div class="advbox col tight" hidden></div>');
          adv.forEach((r) => { const n = rowEl(r, true); rowsEls.push([r, n]); advBox.append(n); });
          disc.onclick = () => { advBox.hidden = !advBox.hidden; disc.classList.toggle('open', !advBox.hidden); };
          gb.append(disc, advBox);
          card._disc = disc;
        }
        $('.ghead', card).onclick = () => card.classList.toggle('open');
        cards.push({ card, rowsEls, advBox, c });
        secEl.append(card);
      });
      body.append(secEl);
    });
    const scroller = wrap;
    const chips = $$('[data-s]', wrap);
    chips.forEach((ch) => (ch.onclick = () => {
      const target = $(`[data-sec="${ch.dataset.s}"]`, wrap);
      const stickyH = $('.sticky', wrap).offsetHeight;
      scroller.scrollTo({ top: target.offsetTop - stickyH - 4, behavior: 'smooth' });
    }));
    const spy = () => {
      const stickyH = $('.sticky', wrap).offsetHeight;
      const vis = $$('.navsec', wrap).filter((s) => !s.hidden);
      let cur = vis.length ? vis[0].dataset.sec : SECS[0];
      vis.forEach((s) => { if (s.offsetTop - stickyH - 12 <= scroller.scrollTop) cur = s.dataset.sec; });
      chips.forEach((c) => c.classList.toggle('on', c.dataset.s === cur));
    };
    scroller.addEventListener('scroll', spy, { passive: true });
    const search = $('.search', wrap), count = $('.scount', wrap);
    search.addEventListener('input', () => {
      const q = search.value.trim().toLowerCase();
      let hits = 0;
      cards.forEach(({ card, rowsEls, advBox, c }) => {
        if (!q) {
          rowsEls.forEach(([, n]) => (n.hidden = false));
          card.hidden = false;
          card.classList.toggle('open', !!c.open);
          if (advBox) { advBox.hidden = true; card._disc.classList.remove('open'); card._disc.hidden = false; }
          return;
        }
        let any = false, advHit = false;
        rowsEls.forEach(([r, n]) => {
          const m = rowText(r).toLowerCase().includes(q) || c.title.toLowerCase().includes(q);
          n.hidden = !m;
          if (m) { any = true; hits++; if (r.adv) advHit = true; }
          n.classList.toggle('hit', m && !c.title.toLowerCase().includes(q));
        });
        card.hidden = !any;
        card.classList.toggle('open', any);
        if (advBox) { advBox.hidden = !advHit; card._disc.hidden = true; }
      });
      $$('.navsec', wrap).forEach((s) => (s.hidden = !$$('.gcard', s).some((c) => !c.hidden)));
      count.textContent = q ? `${hits} match${hits === 1 ? '' : 'es'}` : '';
      scroller.scrollTop = 0;
      spy();
    });
    search.addEventListener('keydown', (e) => { if (e.key === 'Escape') { search.value = ''; search.dispatchEvent(new Event('input')); } });
    st.append(wrap);
    spy();
  };

  // ------------------------------------------------------------ 9 histogram
  const HIST = [[3542,7682,4423,3793,4329,4392,4200,3840,2871,2482,2499,3069,3427,3293,3376,3400,3359,3376,3118,3261,3203,3153,3032,3236,3622,4207,4267,3371,3335,2800,2317,2205,1910,1582,1355,1186,996,889,727,631,581,537,515,487,505,526,512,567,608,838,908,961,966,866,899,934,1067,1241,1070,1035,1317,1485,1466,1353],[7804,8447,5391,5155,5142,5622,6124,6206,5986,5327,4685,3947,4010,4036,4361,4520,4483,4347,4867,4202,3726,3550,3289,3089,2905,2680,2334,2107,1988,1702,1573,1491,1237,1011,905,797,633,522,353,287,235,231,178,155,138,142,113,89,108,93,106,89,70,59,41,35,45,45,45,42,42,21,18,19],[13294,9315,6370,6007,5624,5809,5843,5911,5493,4669,3806,3507,3470,3558,3532,3575,3692,3967,4162,3874,3326,3679,3572,3024,2639,2497,2254,1985,1862,1665,1502,1252,1103,967,788,662,570,506,402,357,320,244,244,223,188,167,126,108,125,95,95,69,84,81,78,70,79,64,81,68,86,70,58,87]];
  const REG = ['Blacks', 'Shadows', 'Exposure', 'Highlights', 'Whites'];
  function tone(x, p) {
    let y = x * Math.pow(2, p.exposure * 0.75);
    const g = (c, w) => Math.exp(-(((y - c) / w) ** 2));
    y += (p.shadows / 100) * 0.16 * g(0.28, 0.16);
    y += (p.highlights / 100) * 0.16 * g(0.72, 0.16);
    const ss = (a, b, t) => { const u = clamp((t - a) / (b - a), 0, 1); return u * u * (3 - 2 * u); };
    y += (p.blacks / 100) * 0.1 * (1 - ss(0, 0.35, y));
    y += (p.whites / 100) * 0.1 * ss(0.65, 1, y);
    return clamp(y, 0, 1);
  }
  let histUid = 0;
  function histPane(st, proposed) {
    const id = `h${histUid++}`;
    const toneFns = SP.toneFilter(`${id}-tone`);
    const hiFns = SP.clipFilter(`${id}-hi`, 'hi', true);
    const loFns = SP.clipFilter(`${id}-lo`, 'lo', true);
    const p = { blacks: 0, shadows: 0, exposure: 0, highlights: 0, whites: 0 };
    let clip = 0;
    const view = el(`<div class="histview"><svg viewBox="0 0 876 648" preserveAspectRatio="xMidYMid slice"><image href="${IMG.car}" width="876" height="648" filter="url(#${id}-tone)"/><image class="ovlo" href="${IMG.car}" width="876" height="648" filter="url(#${id}-lo)" visibility="hidden"/><image class="ovhi" href="${IMG.car}" width="876" height="648" filter="url(#${id}-hi)" visibility="hidden"/></svg></div>`);
    const box = el(`<div class="histbox ${proposed ? 'new' : ''}"><canvas></canvas>${proposed ? '<div class="hband"><span></span></div><button class="tri-btn lo" title="Shadow clipping (J)"></button><button class="tri-btn hi" title="Highlight clipping (O)"></button>' : ''}</div>`);
    const cv = $('canvas', box);
    const info = proposed
      ? el('<div class="exif"><span>ISO 100</span><span>1/320 s</span><span>f/8</span><span>45 mm</span><span class="faint">XCD 4/45P</span></div>')
      : el('<div class="oldvals tiny faint"></div>');
    if (!proposed) box.title = 'Drag horizontally by region: Blacks · Shadows · Exposure · Highlights · Whites';
    function draw() {
      const table = Array.from({ length: 33 }, (_, i) => tone(i / 32, p).toFixed(4)).join(' ');
      [...toneFns, ...hiFns, ...loFns].forEach((f) => f.setAttribute('tableValues', table));
      const dpr = window.devicePixelRatio || 1;
      const W = box.clientWidth, H = box.clientHeight;
      cv.width = W * dpr; cv.height = H * dpr;
      const c = cv.getContext('2d');
      c.scale(dpr, dpr);
      c.fillStyle = '#101112';
      c.beginPath(); c.roundRect(0, 0, W, H, 6); c.fill();
      const bins = HIST.map((ch) => {
        const out = new Array(64).fill(0);
        // Spread each source bin over the output range it maps to, so
        // stretching tones does not leave a comb of empty bins.
        ch.forEach((n, i) => {
          const y0 = tone(i / 64, p) * 64, y1 = tone((i + 1) / 64, p) * 64;
          const a = Math.min(y0, y1), b = Math.max(y0, y1);
          if (b - a < 1e-6) { out[Math.min(63, Math.floor(a))] += n; return; }
          for (let k = Math.floor(a); k < Math.min(64, Math.ceil(b)); k++) {
            const ov = Math.min(b, k + 1) - Math.max(a, k);
            if (ov > 0) out[Math.min(63, k)] += (n * ov) / (b - a);
          }
          if (b >= 64) out[63] += (n * (b - Math.max(a, 64))) / (b - a);
        });
        return out;
      });
      const total = HIST[0].reduce((a, b) => a + b, 0);
      const sorted = bins.flat().slice().sort((a, b) => a - b);
      const peak = Math.max(1, sorted[Math.floor(sorted.length * 0.97)]);
      const ix = 4, iw = W - 8, ih = H - 8;
      const fills = ['rgba(232,92,72,.28)', 'rgba(102,209,126,.28)', 'rgba(92,150,240,.28)'];
      const lines = ['rgba(240,119,96,.8)', 'rgba(132,224,153,.8)', 'rgba(124,174,255,.8)'];
      bins.forEach((b, ci) => {
        c.beginPath();
        c.moveTo(ix, H - 4);
        b.forEach((n, i) => c.lineTo(ix + (iw * (i + 0.5)) / 64, H - 4 - ih * Math.min(1, n / peak)));
        c.lineTo(ix + iw, H - 4);
        c.closePath();
        c.fillStyle = fills[ci]; c.fill();
        c.strokeStyle = lines[ci]; c.lineWidth = 1; c.stroke();
      });
      if (!proposed) {
        const wx = ix + iw * (224 / 256);
        c.strokeStyle = '#8a919b'; c.beginPath(); c.moveTo(wx, 4); c.lineTo(wx, H - 4); c.stroke();
        c.fillStyle = '#8a919b'; c.font = '9px -apple-system, sans-serif'; c.textAlign = 'right'; c.fillText('SDR white', wx - 3, 12);
        info.textContent = `Basic card (below the fold): Blacks ${p.blacks.toFixed(1)} · Shadows ${p.shadows.toFixed(1)} · Exposure ${p.exposure.toFixed(1)} · Highlights ${p.highlights.toFixed(1)} · Whites ${p.whites.toFixed(1)}`;
      } else {
        const hiClip = bins.some((b) => b[63] / total > 0.004), loClip = bins.every((b) => b[0] / total > 0.004);
        $('.tri-btn.hi', box).classList.toggle('lit', hiClip);
        $('.tri-btn.lo', box).classList.toggle('lit', loClip);
        $('.tri-btn.hi', box).classList.toggle('on', !!(clip & 2));
        $('.tri-btn.lo', box).classList.toggle('on', !!(clip & 1));
      }
      $('.ovhi', view).setAttribute('visibility', clip & 2 ? 'visible' : 'hidden');
      $('.ovlo', view).setAttribute('visibility', clip & 1 ? 'visible' : 'hidden');
    }
    const keys = ['blacks', 'shadows', 'exposure', 'highlights', 'whites'];
    const fmtVal = (k) => (k === 'exposure' ? `${signed(p[k], 2)} EV` : signed(p[k], 0));
    const band = proposed ? $('.hband', box) : null;
    const showBand = (region, dragging) => {
      if (!band) return;
      if (region == null) { band.hidden = true; return; }
      band.hidden = false;
      band.style.left = region * 20 + '%';
      band.classList.toggle('drag', !!dragging);
      $('span', band).textContent = `${REG[region]} ${fmtVal(keys[region])}`;
    };
    let dragging = null;
    if (proposed) {
      box.addEventListener('pointermove', (e) => {
        if (dragging != null || e.target.closest('.tri-btn')) return;
        const r = box.getBoundingClientRect();
        showBand(Math.floor(clamp((e.clientX - r.left) / r.width, 0, 0.999) * 5));
      });
      box.addEventListener('pointerleave', () => { if (dragging == null) showBand(null); });
      $$('.tri-btn', box).forEach((b) => (b.onclick = (e) => { e.stopPropagation(); clip ^= b.classList.contains('hi') ? 2 : 1; draw(); }));
    }
    drag(box, {
      start: (e) => {
        if (e.target.closest('.tri-btn')) return false;
        const r = box.getBoundingClientRect();
        const region = Math.floor(clamp((e.clientX - r.left) / r.width, 0, 0.999) * 5);
        dragging = region;
        showBand(region, true);
        return { region, start: p[keys[region]], w: r.width };
      },
      move: (e, s, c) => {
        const n = (e.clientX - s.clientX) / c.w;
        const k = keys[c.region];
        p[k] = k === 'exposure' ? clamp(c.start + n * 10, -5, 5) : clamp(c.start + n * 200, -100, 100);
        showBand(c.region, true);
        draw();
      },
      end: (e) => {
        dragging = null;
        const r = box.getBoundingClientRect();
        const inside = e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
        showBand(inside ? Math.floor(clamp((e.clientX - r.left) / r.width, 0, 0.999) * 5) : null);
      },
    });
    if (proposed) {
      const reset = el('<button class="btn small">Reset tone</button>');
      reset.onclick = () => { keys.forEach((k) => (p[k] = 0)); draw(); };
      info.append(reset);
    } else {
      onKey(st, (e) => {
        if (e.metaKey || e.ctrlKey) return;
        if (e.key === 'j' || e.key === 'J') { clip ^= 1; draw(); return true; }
        if (e.key === 'o' || e.key === 'O') { clip ^= 2; draw(); return true; }
      });
    }
    if (proposed) onKey(st, (e) => {
      if (e.metaKey || e.ctrlKey) return;
      if (e.key === 'j' || e.key === 'J') { clip ^= 1; draw(); return true; }
      if (e.key === 'o' || e.key === 'O') { clip ^= 2; draw(); return true; }
    });
    st.append(view, box, info);
    showBand(null);
    new ResizeObserver(draw).observe(box);
  }
  demos['hist-before'] = (st) => histPane(st, false);
  demos['hist-after'] = (st) => histPane(st, true);

  // ------------------------------------------------------- 10 colour
  const BANDS = [['Red', 0], ['Orange', 30], ['Yellow', 55], ['Green', 120], ['Aqua', 180], ['Blue', 220], ['Purple', 275], ['Magenta', 320]];
  const hsl = (h, s, l) => `hsl(${((h % 360) + 360) % 360} ${clamp(s, 0, 100)}% ${clamp(l, 0, 100)}%)`;
  function colorModel() {
    return { hsl: BANDS.map(() => ({ h: 0, s: 0, l: 0 })), grade: { shadows: { hue: 0, amt: 0 }, midtones: { hue: 0, amt: 0 }, highlights: { hue: 0, amt: 0 } }, balance: 0 };
  }
  function resultStrip(m) {
    const swatches = BANDS.map(([n, h], i) => {
      const b = m.hsl[i];
      return `<i title="${n}" style="background:${hsl(h + b.h * 0.3, 70 + b.s * 0.3, 50 + b.l * 0.2)}"></i>`;
    }).join('');
    const stops = [];
    for (let i = 0; i <= 10; i++) {
      const L = i / 10;
      const bal = m.balance / 200;
      const ws = Math.max(0, 1 - (L - bal) * 2) ** 2, wh = Math.max(0, (L - bal) * 2 - 1) ** 2, wm = Math.max(0, 1 - ws - wh);
      let r = L * 255, g = L * 255, b = L * 255;
      [['shadows', ws], ['midtones', wm], ['highlights', wh]].forEach(([k, w]) => {
        const gr = m.grade[k];
        const a = (gr.amt / 100) * w * 70;
        const rad = (gr.hue * Math.PI) / 180;
        // Hue direction in an RGB-ish opponent space.
        r += a * Math.cos(rad);
        g += a * Math.cos(rad - 2.094);
        b += a * Math.cos(rad + 2.094);
      });
      stops.push(`rgb(${clamp(r, 0, 255) | 0} ${clamp(g, 0, 255) | 0} ${clamp(b, 0, 255) | 0}) ${i * 10}%`);
    }
    return { swatches, grad: `linear-gradient(90deg, ${stops.join(',')})` };
  }
  function renderResult(box, m) {
    const r = resultStrip(m);
    box.innerHTML = `<div class="res-row"><span class="tiny faint">HSL result</span><div class="swatches">${r.swatches}</div></div><div class="res-row"><span class="tiny faint">Grade</span><div class="tonestrip" style="background:${r.grad}"></div></div>`;
  }
  demos['color-before'] = (st) => {
    const m = colorModel();
    const res = el('<div class="result"></div>');
    let sel = 0;
    const card = el(`<div class="card col tight"><div class="muted tiny2">HSL</div><div class="chips"></div><div class="bandname muted tiny2"></div><div class="bandsliders col tight"></div><hr class="sep"><div class="muted tiny2">Color Grading</div><div class="grading col tight"></div></div>`);
    const chips = $('.chips', card);
    const renderChips = () => {
      chips.innerHTML = BANDS.map(([n, h], i) => { const b = m.hsl[i]; const mod = b.h || b.s || b.l; return `<span class="chip ${i === sel ? 'on' : ''}" data-i="${i}" style="background:${hsl(h, 65, 52)}">${mod ? '<i></i>' : ''}</span>`; }).join('');
      $$('.chip', chips).forEach((c) => (c.onclick = () => { sel = +c.dataset.i; renderBand(); }));
    };
    const renderBand = () => {
      renderChips();
      $('.bandname', card).textContent = BANDS[sel][0];
      const bs = $('.bandsliders', card);
      bs.innerHTML = '';
      const b = m.hsl[sel];
      [['Hue', 'h'], ['Saturation', 's'], ['Luminance', 'l']].forEach(([lab, k]) => {
        bs.append(egSlider({ label: lab, min: -100, max: 100, value: b[k], onChange: (v) => { b[k] = v; renderChips(); renderResult(res, m); } }).row);
      });
    };
    const gd = $('.grading', card);
    [['Shadows', 'shadows'], ['Midtones', 'midtones'], ['Highlights', 'highlights']].forEach(([lab, k]) => {
      gd.append(el(`<div class="faint tiny">${lab}</div>`));
      gd.append(egSlider({ label: 'Hue', min: 0, max: 360, value: 0, onChange: (v) => { m.grade[k].hue = v; renderResult(res, m); } }).row);
      gd.append(egSlider({ label: 'Amount', min: 0, max: 100, value: 0, onChange: (v) => { m.grade[k].amt = v; renderResult(res, m); } }).row);
    });
    gd.append(egSlider({ label: 'Balance', min: -100, max: 100, value: 0, onChange: (v) => { m.balance = v; renderResult(res, m); } }).row);
    renderBand();
    renderResult(res, m);
    st.append(res, card);
  };
  demos['color-after'] = (st) => {
    const m = colorModel();
    const res = el('<div class="result"></div>');
    let tab = 'h';
    const card = el(`<div class="card col tight"><div class="row"><span class="section">Color mixer</span><span class="spacer"></span><span class="seg small tabs"><span data-t="h">Hue</span><span data-t="s">Saturation</span><span data-t="l">Luminance</span></span></div><div class="mixer col tight"></div><hr class="sep"><div class="row"><span class="section">Color grading</span><span class="spacer"></span><span class="tiny faint">drag · double-click resets · ⇧ keeps hue</span></div><div class="wheels"></div><div class="bal"></div></div>`);
    const mixer = $('.mixer', card);
    const renderMixer = () => {
      $$('[data-t]', card).forEach((n) => n.classList.toggle('on', n.dataset.t === tab));
      mixer.innerHTML = '';
      BANDS.forEach(([n, h], i) => {
        const grad = tab === 'h' ? `linear-gradient(90deg, ${hsl(h - 30, 70, 50)}, ${hsl(h, 70, 50)}, ${hsl(h + 30, 70, 50)})`
          : tab === 's' ? `linear-gradient(90deg, ${hsl(h, 5, 50)}, ${hsl(h, 90, 50)})`
          : `linear-gradient(90deg, ${hsl(h, 60, 18)}, ${hsl(h, 60, 50)}, ${hsl(h, 60, 82)})`;
        mixer.append(spSlider({ label: n, min: -100, max: 100, def: 0, value: m.hsl[i][tab], gradient: grad, onChange: (v) => { m.hsl[i][tab] = v; renderResult(res, m); } }).row);
      });
    };
    $$('[data-t]', card).forEach((n) => (n.onclick = () => { tab = n.dataset.t; renderMixer(); }));
    const wheels = $('.wheels', card);
    [['Shadows', 'shadows'], ['Midtones', 'midtones'], ['Highlights', 'highlights']].forEach(([lab, k]) => {
      const w = el(`<div class="wheel-wrap"><div class="wheel"><i class="puck"></i></div><div class="wl">${lab}</div><div class="wv tiny faint"></div></div>`);
      const wheel = $('.wheel', w), puck = $('.puck', w), wv = $('.wv', w);
      const g = m.grade[k];
      const render = () => {
        const rad = (g.hue * Math.PI) / 180, rr = (g.amt / 100) * 50;
        puck.style.left = 50 + rr * Math.cos(rad) + '%';
        puck.style.top = 50 + rr * Math.sin(rad) + '%';
        puck.style.background = g.amt ? hsl(g.hue, 20 + g.amt * 0.7, 55) : '#bbb';
        wv.textContent = g.amt ? `${Math.round(g.hue)}° · ${Math.round(g.amt)}` : 'neutral';
        w.classList.toggle('mod', g.amt > 0);
        renderResult(res, m);
      };
      const set = (e, s) => {
        const r = wheel.getBoundingClientRect();
        const dx = e.clientX - (r.left + r.width / 2), dy = e.clientY - (r.top + r.height / 2);
        const amt = clamp((Math.hypot(dx, dy) / (r.width / 2)) * 100, 0, 100);
        if (!(e.shiftKey && s && s.lockHue)) g.hue = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
        g.amt = amt;
        render();
      };
      drag(wheel, { start: (e) => { const c = { lockHue: g.amt > 0 }; set(e, c); return c; }, move: (e, s, c) => set(e, c) });
      wheel.addEventListener('dblclick', () => { g.hue = 0; g.amt = 0; render(); });
      wheels.append(w);
      render();
    });
    $('.bal', card).append(spSlider({ label: 'Balance', min: -100, max: 100, def: 0, value: 0, onChange: (v) => { m.balance = v; renderResult(res, m); } }).row);
    renderMixer();
    renderResult(res, m);
    st.append(res, card);
  };

  // ------------------------------------------------------------ 11 presets
  const PRESETS = [
    ['Essentials', 'Standard Finish', 'contrast(1.06) saturate(1.06)'],
    ['Essentials', 'Nature', 'saturate(1.35) contrast(1.08) hue-rotate(-6deg)'],
    ['Essentials', 'Portrait', 'saturate(.88) brightness(1.06) contrast(.94) sepia(.1)'],
    ['Essentials', 'Product', 'contrast(1.16) brightness(1.04) saturate(1.04)'],
    ['Creative looks', 'Vivid Color', 'saturate(1.65) contrast(1.12)'],
    ['Creative looks', 'Editorial Muted', 'saturate(.55) contrast(.9) brightness(1.05)'],
    ['Creative looks', 'Film Warm', 'sepia(.35) saturate(1.25) contrast(.94) brightness(1.03)'],
    ['Creative looks', 'Film Cool', 'hue-rotate(14deg) saturate(.8) contrast(.94) brightness(1.03)'],
    ['Creative looks', 'Soft Neutral', 'contrast(.82) saturate(.85) brightness(1.07)'],
    ['Monochrome', 'B&W Fine Art', 'grayscale(1) contrast(1.28)'],
    ['Utility', 'Highlight Recovery', 'brightness(.92) contrast(1.06)'],
  ];
  demos['presets-before'] = (st) => {
    const layout = el(`<div class="preset-layout"><div class="rail"><div class="section">Presets</div></div><div class="pview"><div class="stack"><img class="base" src="${IMG.car}" alt=""></div><div class="tweak"></div></div></div>`);
    const rail = $('.rail', layout), img = $('.base', layout);
    let active = null, tweak = 0;
    const btns = [];
    let cat = '';
    PRESETS.forEach(([c, name, f]) => {
      if (c !== cat) { rail.append(el(`<div class="tiny faint cat">${c.toUpperCase()}</div>`)); cat = c; }
      const b = el(`<button class="pbtn">${name}</button>`);
      b.onclick = () => { active = name; tweak = 0; tw.set(0, true); render(); };
      b.title = 'Click to apply';
      btns.push([b, name]);
      rail.append(b);
    });
    const tw = egSlider({ label: 'Exposure', min: -5, max: 5, value: 0, onChange: (v) => { tweak = v; render(); } });
    $('.tweak', layout).append(el('<div class="tiny faint">Edit after applying:</div>'), tw.row);
    function render() {
      const f = PRESETS.find((p) => p[1] === active);
      img.style.filter = `${f ? f[2] : ''} brightness(${Math.pow(2, tweak * 0.5)})`;
      btns.forEach(([b, name]) => {
        const on = name === active;
        b.classList.toggle('on', on);
        b.textContent = on && Math.abs(tweak) > 0.0005 ? `${name}  *` : name;
      });
    }
    st.append(layout);
    render();
  };
  demos['presets-after'] = (st) => {
    const layout = el(`<div class="preset-layout new"><div class="rail"><div class="row"><span class="section">Presets</span><span class="spacer"></span><span class="tiny faint">hover to preview</span></div><div class="pgrid"></div></div><div class="pview"><div class="stack"><img class="base" src="${IMG.car}" alt=""><img class="over" src="${IMG.car}" alt=""><span class="previewing" hidden></span></div><div class="amount"></div><div class="modchip" hidden></div><div class="tweak"></div></div></div>`);
    const grid = $('.pgrid', layout), over = $('.over', layout), base = $('.base', layout), tag = $('.previewing', layout), chip = $('.modchip', layout);
    let active = null, amount = 100, hover = null, tweak = 0;
    let cat = '';
    PRESETS.forEach(([c, name, f]) => {
      if (c !== cat) { grid.append(el(`<div class="tiny faint cat span2">${c}</div>`)); cat = c; }
      const t = el(`<button class="ptile" title="${name}"><img src="${IMG.car}" alt="" style="filter:${f}"><span>${name}</span></button>`);
      t.onmouseenter = () => { hover = name; render(); };
      t.onmouseleave = () => { hover = null; render(); };
      t.onfocus = t.onmouseenter; t.onblur = t.onmouseleave;
      t.onclick = () => { active = name; amount = 100; am.set(100, true); tweak = 0; tw.set(0, true); render(); };
      grid.append(t);
    });
    const am = spSlider({ label: 'Amount', min: 0, max: 100, def: 100, value: 100, fmt: (v) => `${Math.round(v)}%`, onChange: (v) => { amount = v; render(); } });
    $('.amount', layout).append(am.row);
    const tw = spSlider({ label: 'Exposure', min: -5, max: 5, def: 0, value: 0, decimals: 2, step: 0.05, fmt: (v) => `${signed(v, 2)} EV`, onChange: (v) => { tweak = v; render(); } });
    $('.tweak', layout).append(el('<div class="tiny faint">Edit after applying:</div>'), tw.row);
    function render() {
      const name = hover || active;
      const f = PRESETS.find((p) => p[1] === name);
      const exp = `brightness(${Math.pow(2, tweak * 0.5)})`;
      base.style.filter = exp;
      over.style.filter = f ? `${f[2]} ${exp}` : exp;
      over.style.opacity = f ? (hover ? 1 : amount / 100) : 0;
      tag.hidden = !hover;
      tag.textContent = hover ? `Previewing · ${hover}` : '';
      $$('.ptile', grid).forEach((t) => t.classList.toggle('on', t.title === active));
      $('.amount', layout).classList.toggle('disabled', !active);
      const modified = active && Math.abs(tweak) > 0.0005;
      chip.hidden = !modified;
      if (modified) {
        chip.innerHTML = `<span>${active} · modified</span><button class="btn small">Re-apply</button>`;
        $('button', chip).onclick = () => { tweak = 0; tw.set(0, true); render(); };
      }
    }
    st.append(layout);
    render();
  };

  // ------------------------------------------------------------- 12 history
  const STEPS = [
    { label: 'Imported', p: {}, t: 26 * 60 },
    { label: 'Exposure +0.60', p: { exposure: 0.6 }, t: 24 * 60 },
    { label: 'Highlights −14', p: { exposure: 0.6, highlights: -14 }, t: 23 * 60 },
    { label: 'Shadows +22', p: { exposure: 0.6, highlights: -14, shadows: 22 }, t: 20 * 60 },
    { label: 'Preset: Film Warm', p: { exposure: 0.6, highlights: -14, shadows: 22, sepia: 0.35, saturation: 25, contrast: -12 }, t: 12 * 60 },
    { label: 'Vibrance +15', p: { exposure: 0.6, highlights: -14, shadows: 22, sepia: 0.35, saturation: 25, contrast: -12, vibrance: 15 }, t: 6 * 60 },
    { label: 'Basic · 3 changes', p: { exposure: 0.35, highlights: -30, shadows: 30, sepia: 0.35, saturation: 25, contrast: -4, vibrance: 15 }, t: 2 * 60 },
    { label: 'Vignette −35', p: { exposure: 0.35, highlights: -30, shadows: 30, sepia: 0.35, saturation: 25, contrast: -4, vibrance: 15, vignette: -35 }, t: 40 },
  ];
  const T0 = 1790512345;
  demos['history-before'] = (st) => {
    const ph = photo(IMG.car);
    ph.el.classList.add('ph-strip');
    const box = el(`<div class="card col tight hist-old">
      <div class="plainhead open">▾ Snapshots &amp; history</div>
      <div class="row wrap"><span>Versions</span><span class="sel on">Primary</span><button class="btn small">New virtual copy</button></div>
      <div class="row"><input class="field grow" value=""><button class="btn small" data-save disabled>Save</button></div>
      <div class="snaps col tight"></div>
      <hr class="sep"><div class="tiny faint">${STEPS.length - 1} persistent edit steps</div><div class="hsteps col tight"></div></div>`);
    const steps = $('.hsteps', box);
    STEPS.slice(1).reverse().forEach((s) => {
      const b = el(`<button class="btn small left">Edit · ${T0 - s.t}</button>`);
      b.onclick = () => ph.apply(STEPS[STEPS.indexOf(s) - 1].p);
      steps.append(b);
    });
    const input = $('input', box), save = $('[data-save]', box);
    input.addEventListener('input', () => (save.disabled = !input.value.trim()));
    save.onclick = () => {
      const b = el(`<button class="btn small left">${esc(input.value.trim())}</button>`);
      b.onclick = () => ph.apply(STEPS[STEPS.length - 1].p);
      $('.snaps', box).append(b);
      input.value = ''; save.disabled = true;
    };
    st.append(ph.el, box);
    ph.apply(STEPS[STEPS.length - 1].p);
  };
  demos['history-after'] = (st) => {
    const ph = photo(IMG.car);
    ph.el.classList.add('ph-strip');
    const peek = el('<span class="peek" hidden></span>');
    ph.el.append(peek);
    let cur = STEPS.length - 1;
    const snaps = [];
    const box = el(`<div class="card col tight hist-new">
      <div class="row"><span class="section">History</span><span class="spacer"></span><button class="btn small" data-snap>+ Snapshot</button></div>
      <div class="snapnew row" hidden><input class="field grow" placeholder="Snapshot name" value="Warm v1"><button class="btn small primary" data-ok>Save</button></div>
      <div class="snapchips"></div>
      <ol class="timeline"></ol></div>`);
    const tl = $('.timeline', box);
    function render() {
      tl.innerHTML = '';
      STEPS.slice().reverse().forEach((s) => {
        const i = STEPS.indexOf(s);
        const li = el(`<li class="${i === cur ? 'cur' : ''} ${i > cur ? 'redo' : ''}" title="27 Sep 2026 · ${new Date((T0 - s.t) * 1000).toISOString().slice(11, 19)} UTC"><span class="dot"></span><span class="lab">${esc(s.label)}</span><span class="when">${ago(s.t)}</span></li>`);
        li.onmouseenter = () => { ph.apply(s.p); peek.hidden = i === cur; peek.textContent = `Previewing · ${s.label}`; };
        li.onmouseleave = () => { ph.apply(STEPS[cur].p); peek.hidden = true; };
        li.onclick = () => { cur = i; ph.apply(STEPS[cur].p); peek.hidden = true; render(); };
        tl.append(li);
      });
      const chips = $('.snapchips', box);
      chips.innerHTML = '';
      snaps.forEach((sn) => {
        const c = el(`<button class="snapchip"><img src="${IMG.car}" alt="" style="filter:${filterFor(sn.p)}"><span>${esc(sn.name)}</span><span class="cmpw" title="Compare with current">◐</span></button>`);
        c.onmouseenter = () => { ph.apply(sn.p); peek.hidden = false; peek.textContent = `Previewing · ${sn.name}`; };
        c.onmouseleave = () => { ph.apply(STEPS[cur].p); peek.hidden = true; };
        chips.append(c);
      });
    }
    const nw = $('.snapnew', box), inp = $('input', nw);
    $('[data-snap]', box).onclick = () => { nw.hidden = !nw.hidden; if (!nw.hidden) { inp.focus(); inp.select(); } };
    const saveSnap = () => { if (!inp.value.trim()) return; snaps.push({ name: inp.value.trim(), p: STEPS[cur].p }); nw.hidden = true; inp.value = `Snapshot ${snaps.length + 1}`; render(); };
    $('[data-ok]', box).onclick = saveSnap;
    inp.addEventListener('keydown', (e) => { if (e.key === 'Enter') saveSnap(); if (e.key === 'Escape') nw.hidden = true; });
    snaps.push({ name: 'Before preset', p: STEPS[3].p });
    st.append(ph.el, box);
    render();
    ph.apply(STEPS[cur].p);
  };
})();
