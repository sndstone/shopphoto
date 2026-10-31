/* Feedback mocks: 16 background jobs, 17 toasts, 18 shortcut sheet. */
(() => {
  const { IMG, demos, clamp, $, $$, el, esc, onKey } = SP;

  // ------------------------------------------------------------ 16 jobs
  const JOBS = {
    finish: { title: 'Auto Finish', total: 24, unit: 'photos', ms: 420 },
    models: { title: 'Download ShopPhoto model', total: 100, unit: '%', ms: 90 },
  };
  function jobRunner(onTick) {
    const jobs = {};
    return {
      jobs,
      start(k) {
        if (jobs[k] && jobs[k].state === 'run') return;
        const d = JOBS[k];
        const j = (jobs[k] = { k, ...d, done: 0, state: 'run', t0: performance.now() });
        j.timer = setInterval(() => {
          j.done++;
          if (j.done >= j.total) { j.state = 'done'; clearInterval(j.timer); }
          onTick(j);
        }, d.ms);
        onTick(j);
      },
      cancel(k) {
        const j = jobs[k];
        if (!j || j.state !== 'run') return;
        clearInterval(j.timer);
        j.state = 'cancelled';
        onTick(j);
      },
      running() { return Object.values(jobs).filter((j) => j.state === 'run'); },
    };
  }

  demos['jobs-before'] = (st) => {
    st.innerHTML = `<div class="mockwin menuhost">
      <div class="topbar mini"><span class="brand">Shop<span>Photo</span></span><span class="spacer"></span><span class="badge tall">SDR</span><span class="btn pill small">View</span><span class="btn pill small">Fit</span><span class="btn pill small" data-tools>Tools</span><span class="btn pill small primary">Export</span></div>
      <div class="viewport vp-160"><img class="fitimg" src="${IMG.car}" alt=""></div>
      <div class="card gcard detail"><div class="ghead"><span class="chev old">▶</span><span class="section">DETAIL</span></div><div class="gbody col tight"><div class="tiny muted">AI RAW denoise · ISO 3200</div><div class="dl"></div></div></div>
    </div>
    <div class="sim"><b>Simulate</b><button class="btn small" data-s="finish">Start Auto Finish · 24 photos</button><button class="btn small" data-s="models">Start model download</button></div>`;
    const tools = $('[data-tools]', st), detail = $('.detail', st);
    $('.ghead', detail).onclick = () => detail.classList.toggle('open');
    let menu = null, sub = null;
    const run = jobRunner(() => render());
    function render() {
      const busy = run.running().length > 0;
      tools.textContent = busy ? 'Tools •' : 'Tools';
      const m = run.jobs.models;
      $('.dl', st).innerHTML = m && m.state === 'run' ? `<div class="pbar"><i style="width:${(m.done / m.total) * 100}%"></i><span>Downloading model…</span></div>` : m && m.state === 'done' ? '<div class="tiny muted">Ready · local neural model</div>' : '<button class="btn small">Download ShopPhoto model…</button>';
      if (sub && sub.isConnected) fillBatch(sub);
    }
    function fillBatch(box) {
      const j = run.jobs.finish;
      box.innerHTML = `<div>Prepare selected photographs for review</div><div class="tiny muted">Auto Balance runs separately on every file using its existing profile and look.</div>
        <button class="btn small" ${j && j.state === 'run' ? 'disabled' : ''} data-go>Precompute Auto Finish</button>
        ${j && j.state === 'run' ? `<div class="pbar"><i style="width:${(j.done / j.total) * 100}%"></i><span>${Math.round((j.done / j.total) * 100)}%</span></div><div class="tiny">Analyzing ${j.done} of ${j.total}</div><button class="btn small" data-cancel>Cancel after current analysis</button>` : j ? `<div class="tiny">${j.state === 'done' ? `Auto Finish complete · ${j.total} photos` : 'Cancelled'}</div>` : ''}
        <button class="btn small">Export selected…</button>`;
      const go = $('[data-go]', box); if (go) go.onclick = () => run.start('finish');
      const c = $('[data-cancel]', box); if (c) c.onclick = () => run.cancel('finish');
    }
    tools.onclick = () => {
      if (menu && menu.isConnected) { menu.remove(); return; }
      menu = el(`<div class="menu popup toolsmenu"><div class="mi">External editor <span>▸</span></div><div class="mi">AI masks <span>▸</span></div><div class="mi" data-batch>Batch <span>▸</span></div></div>`);
      st.querySelector('.mockwin').append(menu);
      const tr = tools.getBoundingClientRect(), hr = st.querySelector('.mockwin').getBoundingClientRect();
      menu.style.top = tr.bottom - hr.top + 4 + 'px';
      menu.style.right = hr.right - tr.right + 'px';
      $('[data-batch]', menu).onclick = (e) => {
        e.stopPropagation();
        if (sub && sub.isConnected) { sub.remove(); sub = null; return; }
        sub = el('<div class="menu submenu col tight"></div>');
        menu.append(sub);
        fillBatch(sub);
      };
    };
    $$('[data-s]', st).forEach((b) => (b.onclick = () => run.start(b.dataset.s)));
    render();
  };

  demos['jobs-after'] = (st) => {
    st.innerHTML = `<div class="mockwin menuhost">
      <div class="topbar mini"><span class="brand">Shop<span>Photo</span></span><span class="spacer"></span>
        <button class="activity" hidden title="Background activity"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" class="trk"/><circle cx="12" cy="12" r="9" class="prg"/></svg><span class="acount"></span></button>
        <span class="badge">SDR</span><span class="btn pill small primary">Export</span></div>
      <div class="viewport vp-160"><img class="fitimg" src="${IMG.car}" alt=""><div class="mtoasts"></div></div>
    </div>
    <div class="sim"><b>Simulate</b><button class="btn small" data-s="finish">Start Auto Finish · 24 photos</button><button class="btn small" data-s="models">Start model download</button></div>`;
    const btn = $('.activity', st), prg = $('.prg', st), count = $('.acount', st), toasts = $('.mtoasts', st);
    const C = 2 * Math.PI * 9;
    prg.style.strokeDasharray = C;
    let pop = null, doneFlash = null;
    const run = jobRunner((j) => {
      if (j.state === 'done') {
        const t = el(`<div class="mtoast"><span class="okc">✓</span> ${j.title} complete${j.k === 'finish' ? ' · 2 need review' : ''}</div>`);
        toasts.append(t);
        setTimeout(() => t.classList.add('out'), 3200);
        setTimeout(() => t.remove(), 3600);
        doneFlash = performance.now();
        setTimeout(render, 2100);
      }
      render();
    });
    const fmtT = (ms) => { const s = Math.round(ms / 1000); return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`; };
    function render() {
      const all = Object.values(run.jobs);
      const r = run.running();
      const flashing = doneFlash && performance.now() - doneFlash < 2000 && !r.length;
      btn.hidden = !r.length && !flashing && !(pop && pop.isConnected);
      btn.classList.toggle('done', !!flashing);
      const frac = r.length ? r.reduce((a, j) => a + j.done / j.total, 0) / r.length : 1;
      prg.style.strokeDashoffset = C * (1 - frac);
      count.textContent = flashing ? '✓' : r.length > 1 ? r.length : '';
      if (pop && pop.isConnected) {
        pop.innerHTML = `<div class="row"><span class="section">Activity</span><span class="spacer"></span>${all.some((j) => j.state !== 'run') ? '<button class="linkbtn" data-clear>Clear finished</button>' : ''}</div>` +
          (all.length ? all.map((j) => `<div class="job ${j.state}"><div class="row"><b>${j.title}</b><span class="spacer"></span>${j.state === 'run' ? `<button class="btn small" data-c="${j.k}">Cancel</button>` : `<span class="tiny ${j.state === 'done' ? 'okc' : 'faint'}">${j.state === 'done' ? '✓ Done' : 'Cancelled'}</span>`}</div>
            <div class="pbar thin"><i style="width:${(j.done / j.total) * 100}%"></i></div>
            <div class="tiny faint">${j.unit === '%' ? `${j.done}% · 412 MB` : `${j.done} of ${j.total} photos`} · ${fmtT(performance.now() - j.t0)}${j.k === 'finish' && j.state === 'done' ? ' · <a href="#" onclick="return false">Review 2 items</a>' : ''}</div></div>`).join('') : '<div class="faint tiny">No background work</div>');
        $$('[data-c]', pop).forEach((b) => (b.onclick = () => run.cancel(b.dataset.c)));
        const cl = $('[data-clear]', pop);
        if (cl) cl.onclick = () => { Object.keys(run.jobs).forEach((k) => run.jobs[k].state !== 'run' && delete run.jobs[k]); render(); };
      }
    }
    btn.onclick = () => {
      if (pop && pop.isConnected) { pop.remove(); pop = null; render(); return; }
      pop = el('<div class="menu popup actpop"></div>');
      st.querySelector('.mockwin').append(pop);
      const tr = btn.getBoundingClientRect(), hr = st.querySelector('.mockwin').getBoundingClientRect();
      pop.style.top = tr.bottom - hr.top + 6 + 'px';
      pop.style.right = Math.max(6, hr.right - tr.right - 60) + 'px';
      render();
    };
    $$('[data-s]', st).forEach((b) => (b.onclick = () => run.start(b.dataset.s)));
    render();
  };

  // ------------------------------------------------------------ 17 toasts
  function toastStage(st) {
    st.innerHTML = `<div class="mockwin"><div class="viewport vp-260 toastvp"><img class="fitimg dim" src="${IMG.car}" alt=""><div class="tthumbs">${IMG.thumbs.slice(2, 7).map((s, i) => `<img src="${s}" alt="" data-i="${i}" class="${i < 3 ? 'picked' : ''}">`).join('')}</div><div class="toasts"></div></div></div>
      <div class="sim"><b>Trigger</b><button class="btn small" data-t="info">Save preset</button><button class="btn small" data-t="error">Export fails</button><button class="btn small" data-t="trash">Trash 3 selected</button></div>`;
    return $('.toasts', st);
  }
  const MSG = {
    info: 'Saved develop preset “Film Warm tweak”',
    error: 'Could not export presets: Permission denied (os error 13) at /Volumes/Archive/Presets/shopphoto-presets.json',
    trash: 'Moved 3 photo(s) to trash',
  };
  demos['toast-before'] = (st) => {
    const box = toastStage(st);
    const thumbs = $$('.tthumbs img', st);
    $$('[data-t]', st).forEach((b) => (b.onclick = () => {
      const k = b.dataset.t;
      if (k === 'trash') thumbs.slice(0, 3).forEach((t) => t.classList.add('gone'));
      const t = el(`<div class="toast-old ${k === 'error' ? 'err' : ''}">${esc(MSG[k])}</div>`);
      box.append(t);
      setTimeout(() => t.remove(), 6000); // TOAST_TTL_SECS, errors included
    }));
  };
  demos['toast-after'] = (st) => {
    const box = toastStage(st);
    const thumbs = $$('.tthumbs img', st);
    const list = [];
    let paused = false;
    box.addEventListener('pointerenter', () => { paused = true; box.classList.add('paused'); });
    box.addEventListener('pointerleave', () => { paused = false; box.classList.remove('paused'); });
    const TTL = 6000;
    function add(k) {
      const err = k === 'error';
      const t = { k, err, left: TTL, el: null };
      t.el = el(`<div class="toast ${err ? 'err' : ''}"><span class="ticon">${err ? '!' : k === 'trash' ? '🗑' : '✓'}</span><div class="tbody"><div class="tmsg">${esc(k === 'trash' ? 'Moved 3 photos to trash' : MSG[k])}</div>${err ? '<div class="tact"><button class="linkbtn" data-copy>Copy details</button></div>' : ''}</div>${k === 'trash' ? '<button class="tbtn" data-undo>Undo</button>' : ''}<button class="tx" aria-label="Dismiss">×</button>${err ? '' : '<i class="tprog"></i>'}</div>`);
      $('.tx', t.el).onclick = () => remove(t);
      const u = $('[data-undo]', t.el);
      if (u) u.onclick = () => { thumbs.slice(0, 3).forEach((x) => x.classList.remove('gone')); $('.tmsg', t.el).textContent = 'Restored 3 photos'; u.remove(); t.left = 2000; };
      const c = $('[data-copy]', t.el);
      if (c) c.onclick = () => { navigator.clipboard?.writeText(MSG.error).catch(() => {}); c.textContent = 'Copied'; };
      list.push(t);
      box.append(t.el);
      layout();
    }
    function remove(t) {
      t.el.classList.add('out');
      setTimeout(() => t.el.remove(), 180);
      list.splice(list.indexOf(t), 1);
      layout();
    }
    function layout() {
      const more = list.length - 3;
      list.forEach((t, i) => t.el.classList.toggle('collapsed', i < more));
      let pill = $('.tmore', box);
      if (more > 0) {
        if (!pill) { pill = el('<div class="tmore"></div>'); box.prepend(pill); }
        pill.textContent = `+${more} more`;
      } else if (pill) pill.remove();
    }
    let last = performance.now();
    (function tick(now) {
      const dt = now - last;
      last = now;
      if (!paused) list.slice().forEach((t) => {
        if (t.err) return;
        t.left -= dt;
        const p = $('.tprog', t.el);
        if (p) p.style.transform = `scaleX(${clamp(t.left / TTL, 0, 1)})`;
        if (t.left <= 0) remove(t);
      });
      requestAnimationFrame(tick);
    })(last);
    $$('[data-t]', st).forEach((b) => (b.onclick = () => {
      if (b.dataset.t === 'trash') thumbs.slice(0, 3).forEach((t) => t.classList.add('gone'));
      add(b.dataset.t);
    }));
  };

  // ---------------------------------------------------------- 18 shortcuts
  const OLD_ROWS = [['⌘O', 'Open RAW'], ['E', 'Export'], ['D / G', 'Develop / Grid'], ['0–5', 'Set rating'], ['P / X', 'Pick / Reject'], ['← / →', 'Previous / next'], ['Z', 'Fit / 100%'], ['\\', 'Before / after'], ['Tab', 'Hide panels'], ['F12', 'Performance overlay']];
  const GROUPS = [
    ['Navigation', [
      [['⌘O'], 'Open RAW…'], [['D'], 'Develop'], [['G'], 'Library grid'], [['←', '→'], 'Previous / next photo'],
      [['C'], 'Compare 2–4 selected photos'], [['Esc'], 'Leave compare · cancel tool'], [['Tab'], 'Hide side panels (when nothing has focus)'],
    ]],
    ['View', [
      [['Z'], 'Fit ↔ 100%'], [['\\'], 'Before / after'], [['Y'], 'Cycle compare mode', 'new'], [['J'], 'Shadow clipping overlay'], [['O'], 'Highlight clipping overlay'],
      [['M'], 'Show AI mask coverage'], [['F12'], 'Performance overlay'],
    ]],
    ['Rating & labels', [
      [['0', '1', '2', '3', '4', '5'], 'Set rating'], [['P'], 'Pick'], [['X'], 'Reject'], [['U'], 'Unflag'],
      [['6'], 'Red label'], [['7'], 'Yellow label'], [['8'], 'Green label'], [['9'], 'Blue label'], [['⇧9'], 'Purple label'],
    ]],
    ['Editing', [
      [['⌘Z'], 'Undo'], [['⇧⌘Z'], 'Redo'], [['E'], 'Export…'], [['↵'], 'Commit crop / tool', 'new'],
      [['dbl-click'], 'Reset a slider (on its label)'], [['⇧ dbl-click'], 'Auto Exposure (on Exposure)'], [['⌥ drag'], 'Sharpen mask preview (on Masking)'],
    ]],
    ['Help', [[['?'], 'This sheet']]],
  ];
  function keyId(e) {
    const cmd = e.metaKey || e.ctrlKey;
    let k;
    if (e.key === '?') return '?';
    if (/^Key[A-Z]$/.test(e.code)) k = e.code.slice(3);
    else if (/^Digit\d$/.test(e.code)) k = e.code.slice(5);
    else k = { Backslash: '\\', ArrowLeft: '←', ArrowRight: '→', ArrowUp: '↑', ArrowDown: '↓', Escape: 'Esc', Tab: 'Tab', Enter: '↵', F12: 'F12', Space: 'Space' }[e.code] || e.key;
    return (e.shiftKey ? '⇧' : '') + (cmd ? '⌘' : '') + k;
  }
  const IMPLEMENTED = { O: 'toggles highlight clipping', J: 'toggles shadow clipping', C: 'opens the side-by-side compare', M: 'toggles AI mask coverage', U: 'unflags (compare view only)', '6': 'sets a red label', '7': 'sets a yellow label', '8': 'sets a green label', '9': 'sets a blue label', '⇧9': 'sets a purple label', '⌘Z': 'undoes', '⇧⌘Z': 'redoes', '?': 'opens this sheet' };
  const LEAKS = { '⌘E': 'Export (the ⌘ is ignored)', '⌘D': 'Develop (the ⌘ is ignored)', '⌘G': 'Library (the ⌘ is ignored)', '⌘P': 'Pick (the ⌘ is ignored)', '⌘X': 'Reject (the ⌘ is ignored)' };
  demos['keys-before'] = (st) => {
    st.innerHTML = `<div class="modal"><h4>Keyboard shortcuts</h4><div class="kgrid">${OLD_ROWS.map(([k, a]) => `<span class="mono">${esc(k)}</span><span>${esc(a)}</span>`).join('')}</div><button class="btn small">Close</button></div><div class="sim"><b>Last key</b><span class="knote">Press any key while pointing here.</span></div>`;
    const note = $('.knote', st);
    onKey(st, (e) => {
      if (['Shift', 'Meta', 'Control', 'Alt'].includes(e.key)) return;
      const id = keyId(e);
      const listed = OLD_ROWS.find(([k]) => k.split(' / ').some((x) => x === id || (x === '0–5' && /^[0-5]$/.test(id)) || (x === '←' && id === '←') || (x === '→' && id === '→')));
      if (LEAKS[id]) note.innerHTML = `<kbd>${esc(id)}</kbd> also triggers ${LEAKS[id]}.`;
      else if (id === 'Tab') note.innerHTML = '<kbd>Tab</kbd> hides both side panels, even while a button has keyboard focus.';
      else if (listed) note.innerHTML = `<kbd>${esc(id)}</kbd> is on the sheet: ${esc(listed[1])}.`;
      else if (IMPLEMENTED[id]) note.innerHTML = `<kbd>${esc(id)}</kbd> ${IMPLEMENTED[id]} in the app, but <b>is not on this sheet</b>.`;
      else note.innerHTML = `<kbd>${esc(id)}</kbd> does nothing.`;
      return id !== 'Tab';
    });
  };
  demos['keys-after'] = (st) => {
    st.innerHTML = `<div class="modal new"><div class="row"><h4>Keyboard shortcuts</h4><span class="spacer"></span><span class="tiny faint">press a key to find it</span></div><input class="field ksearch" placeholder="Search actions or keys…" data-passkeys-no><div class="kgroups"></div></div>`;
    const box = $('.kgroups', st), input = $('.ksearch', st);
    function render(q = '', hit = null) {
      box.innerHTML = '';
      q = q.trim().toLowerCase();
      let any = false;
      GROUPS.forEach(([g, rows]) => {
        const rs = rows.filter(([keys, a]) => !q || a.toLowerCase().includes(q) || keys.join(' ').toLowerCase().includes(q) || g.toLowerCase().includes(q));
        if (!rs.length) return;
        any = true;
        const sec = el(`<div class="kgroup"><div class="section">${g}</div></div>`);
        rs.forEach(([keys, a, tag]) => {
          const isHit = hit && keys.includes(hit);
          const row = el(`<div class="krow ${isHit ? 'hit' : ''}"><span>${esc(a)}${tag ? ' <span class="newtag">new</span>' : ''}</span><span class="kkeys">${keys.map((k) => `<span class="kbd">${esc(k)}</span>`).join(keys.length > 2 ? '' : '<span class="faint"> / </span>')}</span></div>`);
          sec.append(row);
        });
        box.append(sec);
      });
      if (!any) box.append(el('<div class="faint tiny">No shortcut matches. Try “clip”, “label” or “undo”.</div>'));
      const h = $('.krow.hit', box);
      if (h) box.scrollTo({ top: h.offsetTop - box.offsetTop - 30, behavior: 'smooth' });
    }
    input.addEventListener('input', () => render(input.value));
    onKey(st, (e) => {
      if (['Shift', 'Meta', 'Control', 'Alt'].includes(e.key)) return;
      const id = keyId(e);
      input.value = '';
      render('', id);
      return id !== 'Tab';
    });
    render();
  };
})();
