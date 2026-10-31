/* Library mocks: 13 grid selection/badges, 14 filter bar + selection bar, 15 filmstrip. */
(() => {
  const { IMG, A, demos, clamp, $, $$, el, esc, onKey, cmdKey } = SP;

  const LABELS = { red: '#e5534b', yellow: '#e3b341', green: '#57ab5a', blue: '#539bf5', purple: '#b083f0' };
  const LABEL_KEYS = { 6: 'red', 7: 'yellow', 8: 'green', 9: 'blue' };
  const dataset = () => [
    { n: 'B0002501-external.tif', img: null, ar: 0.75, r: 0, f: null, c: null, e: false, type: 'tif', date: '2026-09-12', cam: 'X2D II 100C', tiff: true },
    { n: 'B0002487.3FR', img: 0, ar: 0.77, r: 4, f: 'pick', c: 'green', e: true, type: '3fr', date: '2026-08-14', cam: 'X2D II 100C' },
    { n: 'B0002491.3FR', img: 1, ar: 0.68, r: 0, f: null, c: null, e: false, type: '3fr', date: '2026-08-14', cam: 'X2D II 100C' },
    { n: 'DSC08812.ARW', img: 2, ar: 1.39, r: 3, f: null, c: null, e: true, type: 'arw', date: '2026-07-02', cam: 'ILCE-7RM5' },
    { n: 'DSC08813.ARW', img: 3, ar: 1.39, r: 2, f: 'reject', c: null, e: false, type: 'arw', date: '2026-07-02', cam: 'ILCE-7RM5' },
    { n: 'x1d-0412.fff', img: 4, ar: 1.39, r: 5, f: 'pick', c: 'red', e: true, type: 'fff', date: '2026-06-20', cam: 'X1D II 50C' },
    { n: 'x1d-0419.fff', img: 5, ar: 0.78, r: 4, f: null, c: 'yellow', e: true, type: 'fff', date: '2026-06-20', cam: 'X1D II 50C' },
    { n: 'B0002502.3FR', img: 6, ar: 1.39, r: 3, f: 'pick', c: null, e: true, type: '3fr', date: '2026-09-12', cam: 'X2D II 100C' },
    { n: 'B0002503.3FR', img: 7, ar: 0.78, r: 1, f: null, c: null, e: false, type: '3fr', date: '2026-09-12', cam: 'X2D II 100C' },
    { n: 'B0002507.3FR', img: 8, ar: 1.39, r: 0, f: null, c: 'blue', e: false, type: '3fr', date: '2026-09-12', cam: 'X2D II 100C' },
    { n: 'B0002512.3FR', img: 9, ar: 1.39, r: 2, f: 'reject', c: null, e: false, type: '3fr', date: '2026-09-13', cam: 'X2D II 100C' },
    { n: 'B0002513.3FR', img: 10, ar: 1.39, r: 0, f: null, c: null, e: false, type: '3fr', date: '2026-09-13', cam: 'X2D II 100C' },
  ];
  const src = (p) => (p.img == null ? null : IMG.thumbs[p.img]);
  const starsText = (r) => (r === 0 ? '☆☆☆☆☆' : '★'.repeat(r) + '☆'.repeat(5 - r));

  /** Small popup menu anchored under `anchor`; closes on outside click. */
  function popup(anchor, html, cls = '') {
    closePopups();
    const host = anchor.closest('.menuhost') || anchor.parentElement;
    const m = el(`<div class="menu popup ${cls}">${html}</div>`);
    host.append(m);
    const r = anchor.getBoundingClientRect(), hr = host.getBoundingClientRect();
    m.style.left = r.left - hr.left + 'px';
    m.style.top = r.bottom - hr.top + 4 + 'px';
    const mr = m.getBoundingClientRect();
    if (mr.right > hr.right - 4) m.style.left = Math.max(0, r.right - hr.left - mr.width) + 'px';
    return m;
  }
  function closePopups() { $$('.menu.popup').forEach((m) => m.remove()); }
  document.addEventListener('pointerdown', (e) => { if (!e.target.closest('.menu.popup, [data-pop]')) closePopups(); });
  function combo(label, options, onPick) {
    const c = el(`<span class="combo" data-pop>${esc(label)}</span>`);
    c.onclick = () => {
      if (c._open && c._open.isConnected) { closePopups(); return; }
      const m = popup(c, options.map(([v, t]) => `<div class="mi" data-v="${esc(v)}">${esc(t)}</div>`).join(''));
      $$('[data-v]', m).forEach((mi) => (mi.onclick = () => { onPick(mi.dataset.v, mi.textContent); c.textContent = mi.textContent; closePopups(); }));
      c._open = m;
    };
    return c;
  }

  function developOverlay(host, p, onBack) {
    const ov = el(`<div class="devov"><div class="devbar"><button class="btn small" data-back>← Library <span class="kbd">G</span></button><span class="muted">Develop · ${esc(p.n)}</span></div><div class="devimg">${src(p) ? `<img src="${src(p)}" alt="">` : '<span class="faint">no preview</span>'}</div></div>`);
    $('[data-back]', ov).onclick = () => { ov.remove(); onBack && onBack(); };
    host.append(ov);
    return ov;
  }

  // -------------------------------------------------------------- 13 grid
  demos['grid-before'] = (st) => {
    const P = dataset().slice(1).concat(dataset()[0]);
    let sel = new Set(), open = 7, ov = null;
    const wrap = el('<div class="gridhost"><div class="lgrid old"></div></div>');
    const grid = $('.lgrid', wrap);
    const note = el('<div class="sim"><b>Where keys went</b><span class="keynote">nothing yet</span></div>');
    function render() {
      grid.innerHTML = '';
      P.forEach((p, i) => {
        const badges = `${p.e ? '●' : ''}${p.tiff ? ' TIFF' : ''}${p.f === 'pick' ? ' P' : p.f === 'reject' ? ' X' : ''}`;
        const c = el(`<div class="lcell ${sel.has(i) ? 'is-sel' : ''}" title="${esc(p.n)}"><div class="limg">${src(p) ? `<img src="${src(p)}" alt="">` : ''}</div><div class="lfoot"><span class="lstars">${starsText(p.r)}</span><span class="lbadges">${badges}</span></div></div>`);
        c.onclick = (e) => {
          if (cmdKey(e)) { sel.has(i) ? sel.delete(i) : sel.add(i); render(); return; }
          sel = new Set([i]);
          open = i;
          render();
          ov = developOverlay(wrap, p, () => (ov = null));
        };
        grid.append(c);
      });
    }
    onKey(st, (e) => {
      if (e.metaKey || e.ctrlKey) return;
      if ((e.key === 'g' || e.key === 'G') && ov) { ov.remove(); ov = null; return true; }
      const p = P[open];
      let did = null;
      if (/^[0-5]$/.test(e.key)) { p.r = +e.key; did = `rating ${e.key}`; }
      if (e.key === 'p' || e.key === 'P') { p.f = 'pick'; did = 'Pick'; }
      if (e.key === 'x' || e.key === 'X') { p.f = 'reject'; did = 'Reject'; }
      if (!did) return;
      $('.keynote', note).innerHTML = `${did} → <b>${esc(p.n)}</b>, the photo last opened in Develop. ${sel.size && !sel.has(open) ? `Your ${sel.size} selected photo${sel.size > 1 ? 's were' : ' was'} not changed.` : ''}`;
      render();
      return true;
    });
    render();
    st.append(wrap, note);
  };

  demos['grid-after'] = (st) => {
    const P = dataset().slice(1).concat(dataset()[0]);
    let sel = new Set(), focus = null, anchor = null, ov = null;
    const wrap = el('<div class="gridhost"><div class="lgrid new" tabindex="0" aria-label="Photo grid"></div><div class="gstatus"></div></div>');
    const grid = $('.lgrid', wrap), status = $('.gstatus', wrap);
    let msgT;
    const say = (m) => { status.innerHTML = m; status.classList.add('show'); clearTimeout(msgT); msgT = setTimeout(() => status.classList.remove('show'), 2200); };
    const cols = () => { const c = grid.firstElementChild; if (!c) return 4; return Math.max(1, Math.round(grid.clientWidth / (c.offsetWidth + 8))); };
    function render() {
      grid.innerHTML = '';
      P.forEach((p, i) => {
        const c = el(`<div class="lcell ${sel.has(i) ? 'is-sel' : ''} ${focus === i ? 'focus' : ''} ${p.f === 'reject' ? 'rej' : ''}" title="${esc(p.n)}">
          <div class="limg">${src(p) ? `<img src="${src(p)}" alt="">` : '<span class="ltype">TIFF</span>'}
            ${p.f === 'pick' ? '<span class="flag pick">✓</span>' : p.f === 'reject' ? '<span class="flag rej">✕</span>' : ''}
            ${p.e ? '<span class="edited" title="Edited">◐</span>' : ''}
          </div>
          <div class="lfoot"><span class="lstars2 ${p.r ? '' : 'empty'}">${[1, 2, 3, 4, 5].map((n) => `<i data-r="${n}" class="${n <= p.r ? 'on' : ''}">★</i>`).join('')}</span><span class="lname">${esc(p.n)}</span></div>
          ${p.c ? `<span class="lbar" style="background:${LABELS[p.c]}"></span>` : ''}
        </div>`);
        c.addEventListener('click', (e) => {
          const star = e.target.closest('[data-r]');
          if (star) {
            const n = +star.dataset.r;
            const targets = sel.has(i) ? [...sel] : [i];
            targets.forEach((t) => (P[t].r = P[t].r === n && targets.length === 1 ? 0 : n));
            say(`Rated ${targets.length} photo${targets.length > 1 ? 's' : ''} ${'★'.repeat(n)}`);
            render();
            return;
          }
          if (e.shiftKey && anchor != null) {
            const [a, b] = [Math.min(anchor, i), Math.max(anchor, i)];
            if (!cmdKey(e)) sel = new Set();
            for (let k = a; k <= b; k++) sel.add(k);
          } else if (cmdKey(e)) {
            sel.has(i) ? sel.delete(i) : sel.add(i);
            anchor = i;
          } else {
            sel = new Set([i]);
            anchor = i;
          }
          focus = i;
          render();
          grid.focus({ preventScroll: true });
        });
        c.addEventListener('dblclick', () => openAt(i));
        grid.append(c);
      });
    }
    function openAt(i) {
      ov = developOverlay(wrap, P[i], () => { ov = null; grid.focus({ preventScroll: true }); });
    }
    onKey(st, (e) => {
      if (ov) { if (e.key === 'g' || e.key === 'G' || e.key === 'Escape') { $('[data-back]', ov).click(); return true; } return; }
      const targets = [...sel];
      const n = P.length;
      const move = (d) => {
        const nf = clamp((focus ?? -1) + d, 0, n - 1);
        if (e.shiftKey) { if (anchor == null) anchor = focus ?? nf; sel = new Set(); const [a, b] = [Math.min(anchor, nf), Math.max(anchor, nf)]; for (let k = a; k <= b; k++) sel.add(k); }
        else { sel = new Set([nf]); anchor = nf; }
        focus = nf;
        render();
      };
      if (e.key === 'ArrowRight') { move(1); return true; }
      if (e.key === 'ArrowLeft') { move(-1); return true; }
      if (e.key === 'ArrowDown') { move(cols()); return true; }
      if (e.key === 'ArrowUp') { move(-cols()); return true; }
      if (e.key === 'Enter' && focus != null) { openAt(focus); return true; }
      if (e.key === 'Escape') { sel = new Set(); render(); return true; }
      if ((e.key === 'a' || e.key === 'A') && cmdKey(e)) { sel = new Set(P.map((_, i) => i)); render(); return true; }
      if (e.metaKey || e.ctrlKey || !targets.length) return;
      const plural = `${targets.length} photo${targets.length > 1 ? 's' : ''}`;
      if (/^[0-5]$/.test(e.key)) { targets.forEach((t) => (P[t].r = +e.key)); say(`Rated ${plural} ${+e.key ? '★'.repeat(+e.key) : '(no stars)'}`); }
      else if (e.key === 'p' || e.key === 'P') { targets.forEach((t) => (P[t].f = 'pick')); say(`<span class="okc">✓</span> Picked ${plural}`); }
      else if (e.key === 'x' || e.key === 'X') { targets.forEach((t) => (P[t].f = 'reject')); say(`<span class="errc">✕</span> Rejected ${plural}`); }
      else if (e.key === 'u' || e.key === 'U') { targets.forEach((t) => (P[t].f = null)); say(`Unflagged ${plural}`); }
      else if (LABEL_KEYS[e.key]) {
        const lab = LABEL_KEYS[e.key];
        const all = targets.every((t) => P[t].c === lab);
        targets.forEach((t) => (P[t].c = all ? null : lab));
        say(`${all ? 'Cleared' : 'Labelled'} ${plural} <i class="dotc" style="background:${LABELS[lab]}"></i>`);
      } else return;
      render();
      return true;
    });
    render();
    st.append(wrap);
  };

  // --------------------------------------------------------- 14 filters
  function applyFilters(P, f) {
    return P.map((p, i) => [p, i]).filter(([p]) =>
      p.r >= (f.rating || 0) &&
      (f.flag == null || (f.flag === 'none' ? !p.f : p.f === f.flag)) &&
      (!f.color || (f.color === 'none' ? !p.c : p.c === f.color)) &&
      (!f.type || p.type === f.type) &&
      (!f.cam || p.cam === f.cam) &&
      (!f.text || `${p.n} ${p.cam}`.toLowerCase().includes(f.text.toLowerCase())));
  }
  function miniGrid(host, P, list, sel, onClick) {
    host.innerHTML = '';
    if (!list.length) { host.append(el('<div class="empty muted">No photos match the current filters</div>')); return; }
    list.forEach(([p, i]) => {
      const c = el(`<div class="mcell ${sel.has(i) ? 'is-sel' : ''} ${p.f === 'reject' ? 'rej' : ''}" title="${esc(p.n)}">${src(p) ? `<img src="${src(p)}" alt="">` : '<span class="ltype">TIFF</span>'}<span class="mstars">${p.r ? '★'.repeat(p.r) : ''}</span>${p.c ? `<i class="mdot" style="background:${LABELS[p.c]}"></i>` : ''}</div>`);
      c.onclick = (e) => onClick(i, e);
      host.append(c);
    });
  }

  demos['filter-before'] = (st) => {
    const P = dataset();
    const f = { rating: 0, flag: null, color: null, type: null, text: '' };
    let sel = new Set();
    const bar = el(`<div class="lbar-old menuhost">
      <div class="lrow wrap"><input class="field" style="width:150px" placeholder="Search photos"><span class="vsep"></span><span class="muted">Rating ≥</span><span class="rsel">${[0, 1, 2, 3, 4, 5].map((n) => `<span class="sel ${n === 0 ? 'on' : ''}" data-r="${n}">${n}</span>`).join('')}</span><span class="vsep"></span></div>
      <div class="lrow wrap sub"><span class="nsel">0 selected</span><span class="muted">Rename pattern</span><input class="field" style="width:140px" placeholder="{date}-{name}-{seq}"><button class="btn small" data-op disabled>Rename</button><button class="btn small" data-op disabled>Move…</button><button class="btn small" data-op disabled>Trash…</button><button class="btn small" disabled>Undo file move</button></div>
    </div>`);
    const r1 = $('.lrow', bar);
    r1.append(
      combo('Any flag', [['', 'Any flag'], ['pick', 'Picked'], ['reject', 'Rejected'], ['none', 'Unflagged']], (v) => { f.flag = v || null; render(); }),
      combo('ALL RAWS', [['', 'All RAWs'], ['3fr', '3FR'], ['fff', 'FFF'], ['arw', 'ARW'], ['dng', 'DNG']], (v) => { f.type = v || null; render(); }),
      combo('Any color', [['', 'Any color'], ['none', 'No color'], ['red', 'Red'], ['yellow', 'Yellow'], ['green', 'Green'], ['blue', 'Blue'], ['purple', 'Purple']], (v) => { f.color = v || null; render(); }),
      el('<span class="btn small ghost" data-pop>Metadata filters</span>'),
      el('<span class="btn small ghost" data-pop>Smart collections</span>'),
      combo('Capture time', [['t', 'Capture time'], ['n', 'Name'], ['r', 'Rating']], () => {}),
    );
    $$('.btn.ghost', r1).forEach((b) => (b.onclick = () => {
      popup(b, b.textContent.startsWith('Meta')
        ? '<div class="mi">Any camera ▼</div><div class="mi">Any lens ▼</div><div class="mi">ISO  min 0  max 0</div><div class="mi">Date  YYYY-MM-DD  YYYY-MM-DD</div><div class="mi faint">Clear metadata filters</div>'
        : '<div class="mi">Picks 2026</div><div class="mi">X2D keepers</div><hr><div class="mi faint">[ name ]  Save current filter</div>');
    }));
    $$('[data-r]', bar).forEach((s) => (s.onclick = () => { f.rating = +s.dataset.r; $$('[data-r]', bar).forEach((x) => x.classList.toggle('on', x === s)); render(); }));
    $('input', bar).addEventListener('input', (e) => { f.text = e.target.value; render(); });
    const grid = el('<div class="mgrid"></div>');
    function render() {
      const list = applyFilters(P, f);
      miniGrid(grid, P, list, sel, (i, e) => { if (cmdKey(e)) { sel.has(i) ? sel.delete(i) : sel.add(i); } else sel = new Set([i]); render(); });
      $('.nsel', bar).textContent = `${sel.size} selected`;
      $$('[data-op]', bar).forEach((b) => (b.disabled = !sel.size));
    }
    render();
    st.append(bar, grid);
  };

  demos['filter-after'] = (st) => {
    const P = dataset();
    const f = { rating: 0, flag: null, color: null, type: null, cam: null, text: '' };
    let sel = new Set(), lastRename = null;
    const host = el(`<div class="menuhost lnew">
      <div class="fbar">
        <label class="fsearch"><span>⌕</span><input placeholder="Search" data-passkeys></label>
        <span class="fstars" title="Minimum rating">${[1, 2, 3, 4, 5].map((n) => `<i data-r="${n}">★</i>`).join('')}</span>
        <span class="fflags"><button data-f="pick" title="Picked">✓</button><button data-f="reject" title="Rejected">✕</button><button data-f="none" title="Unflagged">○</button></span>
        <span class="fcolors">${Object.entries(LABELS).map(([k, c]) => `<button data-c="${k}" title="${k}" style="--c:${c}"></button>`).join('')}</span>
        <button class="btn small ghost" data-pop data-more>More ▾</button>
        <span class="spacer"></span>
        <span class="fsort"></span>
      </div>
      <div class="chips"><span class="chiplist"></span><span class="spacer"></span><span class="count faint"></span><button class="linkbtn" data-clear>Clear</button></div>
      <div class="mgrid"></div>
      <div class="selbar"><span class="nsel"></span><button class="btn small" data-pop data-rename>Rename…</button><button class="btn small" data-move>Move…</button><button class="btn small danger" data-trash>Trash…</button><span class="spacer"></span><button class="gbtn" data-x title="Clear selection">✕</button></div>
    </div>`);
    const grid = $('.mgrid', host), chips = $('.chiplist', host), selbar = $('.selbar', host);
    $('.fsort', host).append(combo('Capture time', [['t', 'Capture time'], ['n', 'Name'], ['r', 'Rating']], () => {}));
    $('input', host).addEventListener('input', (e) => { f.text = e.target.value; render(); });
    $$('[data-r]', host).forEach((s) => (s.onclick = () => { f.rating = f.rating === +s.dataset.r ? 0 : +s.dataset.r; render(); }));
    $$('[data-f]', host).forEach((b) => (b.onclick = () => { f.flag = f.flag === b.dataset.f ? null : b.dataset.f; render(); }));
    $$('[data-c]', host).forEach((b) => (b.onclick = () => { f.color = f.color === b.dataset.c ? null : b.dataset.c; render(); }));
    $('[data-more]', host).onclick = (e) => {
      const m = popup(e.currentTarget, `<div class="section pad">File type</div>${['3fr', 'fff', 'arw', 'tif'].map((t) => `<div class="mi" data-t="${t}">${t.toUpperCase()}${f.type === t ? ' ✓' : ''}</div>`).join('')}<hr><div class="section pad">Camera</div>${['X2D II 100C', 'X1D II 50C', 'ILCE-7RM5'].map((c) => `<div class="mi" data-cam="${c}">${c}${f.cam === c ? ' ✓' : ''}</div>`).join('')}`);
      $$('[data-t]', m).forEach((mi) => (mi.onclick = () => { f.type = f.type === mi.dataset.t ? null : mi.dataset.t; closePopups(); render(); }));
      $$('[data-cam]', m).forEach((mi) => (mi.onclick = () => { f.cam = f.cam === mi.dataset.cam ? null : mi.dataset.cam; closePopups(); render(); }));
    };
    $('[data-clear]', host).onclick = () => { Object.assign(f, { rating: 0, flag: null, color: null, type: null, cam: null, text: '' }); $('input', host).value = ''; render(); };
    $('[data-x]', host).onclick = () => { sel = new Set(); render(); };
    $('[data-trash]', host).onclick = () => { const n = sel.size; sel = new Set(); render(); flash(`Moved ${n} photo${n > 1 ? 's' : ''} to trash (mock)`); };
    $('[data-move]', host).onclick = () => flash('Choose destination… (mock)');
    $('[data-rename]', host).onclick = (e) => {
      const m = popup(e.currentTarget, `<div class="rename"><div class="section pad">Rename ${sel.size} photo${sel.size > 1 ? 's' : ''}</div><input class="field" value="{date}-{name}-{seq}" data-passkeys><div class="tokens">${['{date}', '{name}', '{seq}', '{camera}'].map((t) => `<button class="tok">${t}</button>`).join('')}</div><div class="preview"></div><div class="row"><span class="spacer"></span><button class="btn small primary" data-go>Rename</button></div></div>`, 'above');
      const inp = $('input', m), pv = $('.preview', m);
      const names = () => [...sel].sort((a, b) => a - b).map((i, k) => {
        const p = P[i], ext = p.n.split('.').pop();
        const base = inp.value.replace(/\{date\}/g, p.date).replace(/\{name\}/g, p.n.replace(/\.[^.]+$/, '')).replace(/\{seq\}/g, String(k + 1).padStart(3, '0')).replace(/\{camera\}/g, p.cam.replace(/\s+/g, ''));
        return [i, p.n, `${base}.${ext}`];
      });
      const upd = () => { pv.innerHTML = names().slice(0, 3).map(([, a, b]) => `<div class="rn"><span class="faint">${esc(a)}</span><span>→</span><span>${esc(b)}</span></div>`).join('') + (sel.size > 3 ? `<div class="faint tiny">…and ${sel.size - 3} more</div>` : ''); };
      inp.addEventListener('input', upd);
      $$('.tok', m).forEach((t) => (t.onclick = () => { inp.value += (inp.value && !inp.value.endsWith('-') ? '-' : '') + t.textContent; upd(); inp.focus(); }));
      $('[data-go]', m).onclick = () => { lastRename = names().map(([i, a]) => [i, a]); names().forEach(([i, , b]) => (P[i].n = b)); closePopups(); render(); flash(`Renamed ${lastRename.length} files`, () => { lastRename.forEach(([i, a]) => (P[i].n = a)); render(); }); };
      upd();
    };
    const note = el('<div class="fnote"></div>');
    function flash(msg, undo) {
      note.innerHTML = `<span>${esc(msg)}</span>${undo ? '<button class="linkbtn">Undo</button>' : ''}`;
      note.classList.add('show');
      if (undo) $('button', note).onclick = () => { undo(); note.classList.remove('show'); };
      clearTimeout(note._t);
      note._t = setTimeout(() => note.classList.remove('show'), 4000);
    }
    host.append(note);
    function render() {
      $$('[data-r]', host).forEach((s) => s.classList.toggle('on', +s.dataset.r <= f.rating));
      $$('[data-f]', host).forEach((b) => b.classList.toggle('on', f.flag === b.dataset.f));
      $$('[data-c]', host).forEach((b) => b.classList.toggle('on', f.color === b.dataset.c));
      const active = [];
      if (f.text) active.push(['text', `“${f.text}”`]);
      if (f.rating) active.push(['rating', `★ ≥ ${f.rating}`]);
      if (f.flag) active.push(['flag', { pick: '✓ Picked', reject: '✕ Rejected', none: 'Unflagged' }[f.flag]]);
      if (f.color) active.push(['color', `<i class="dotc" style="background:${LABELS[f.color]}"></i> ${f.color}`]);
      if (f.type) active.push(['type', `Type ${f.type.toUpperCase()}`]);
      if (f.cam) active.push(['cam', f.cam]);
      chips.innerHTML = active.length ? active.map(([k, t]) => `<span class="fchip" data-k="${k}">${t}<button aria-label="Remove">×</button></span>`).join('') : '<span class="faint tiny">No filters</span>';
      $$('.fchip button', chips).forEach((b) => (b.onclick = () => {
        const k = b.parentElement.dataset.k;
        if (k === 'text') { f.text = ''; $('input', host).value = ''; } else if (k === 'rating') f.rating = 0; else f[k] = null;
        render();
      }));
      const list = applyFilters(P, f);
      $('.count', host).textContent = `Showing ${list.length} of ${P.length}`;
      $('[data-clear]', host).hidden = !active.length;
      const visible = new Set(list.map(([, i]) => i));
      sel = new Set([...sel].filter((i) => visible.has(i)));
      miniGrid(grid, P, list, sel, (i, e) => {
        if (cmdKey(e) || e.shiftKey) { sel.has(i) ? sel.delete(i) : sel.add(i); } else sel = sel.size === 1 && sel.has(i) ? new Set() : new Set([i]);
        render();
      });
      selbar.classList.toggle('show', sel.size > 0);
      $('.nsel', host).textContent = `${sel.size} selected`;
    }
    render();
    st.append(host);
  };

  // ----------------------------------------------------------- 15 filmstrip
  function stripPreview(p) {
    return src(p) ? `<img src="${src(p)}" alt="">` : `<span class="faint">${esc(p.n)} · preview not generated</span>`;
  }
  demos['strip-before'] = (st) => {
    const P = dataset();
    let cur = 1, sel = new Set([1]);
    const view = el('<div class="stripview"></div>');
    const strip = el('<div class="fstrip old"></div>');
    function render() {
      view.innerHTML = stripPreview(P[cur]);
      strip.innerHTML = '';
      P.forEach((p, i) => {
        const c = el(`<div class="fcell ${i === cur ? 'cur' : ''} ${sel.has(i) ? 'is-sel' : ''}" title="${esc(p.n)}">${src(p) ? `<img src="${src(p)}" alt="">` : `<span class="fname">${esc(p.n)}</span>`}</div>`);
        c.onclick = (e) => {
          if (cmdKey(e)) { sel.has(i) ? sel.delete(i) : sel.add(i); }
          else { sel = new Set([i]); cur = i; }
          render();
        };
        strip.append(c);
      });
    }
    onKey(st, (e) => {
      if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
        cur = clamp(cur + (e.key === 'ArrowRight' ? 1 : -1), 0, P.length - 1);
        render();
        strip.children[cur].scrollIntoView({ block: 'nearest', inline: 'nearest' });
        return true;
      }
    });
    render();
    st.append(view, strip);
  };
  demos['strip-after'] = (st) => {
    const P = dataset();
    let cur = 1, sel = new Set(), tiffLoaded = false;
    const view = el('<div class="stripview"></div>');
    const bar = el('<div class="fstrip-wrap"><div class="fnav"><button class="gbtn" data-p>‹</button><span class="fcount"></span><button class="gbtn" data-n>›</button></div><div class="fstrip new"></div></div>');
    const strip = $('.fstrip', bar);
    const go = (i) => { cur = clamp(i, 0, P.length - 1); render(); strip.children[cur].scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'smooth' }); };
    $('[data-p]', bar).onclick = () => go(cur - 1);
    $('[data-n]', bar).onclick = () => go(cur + 1);
    function render() {
      const p0 = P[cur];
      view.innerHTML = tiffLoaded || !p0.tiff ? (p0.tiff ? `<img src="${IMG.photo}" alt="">` : stripPreview(p0)) : '<span class="faint">Rendering preview…</span>';
      $('.fcount', bar).innerHTML = `<b>${cur + 1}</b> / ${P.length}${sel.size > 1 ? ` · ${sel.size} sel.` : ''}`;
      strip.innerHTML = '';
      P.forEach((p, i) => {
        const loading = p.tiff && !tiffLoaded;
        const img = p.tiff ? (tiffLoaded ? IMG.photo : null) : src(p);
        const c = el(`<div class="fcell ${i === cur ? 'cur' : ''} ${sel.has(i) ? 'is-sel' : ''} ${p.f === 'reject' ? 'rej' : ''} ${loading ? 'loading' : ''}" style="--ar:${p.ar}" title="${esc(p.n)}">
          ${img ? `<img src="${img}" alt="">` : ''}
          ${p.tiff ? '<span class="ftype">TIFF</span>' : ''}
          ${p.f === 'pick' ? '<span class="fflag pick">✓</span>' : p.f === 'reject' ? '<span class="fflag rej">✕</span>' : ''}
          ${p.e ? '<span class="fed"></span>' : ''}
          ${p.r ? `<span class="fst">${'★'.repeat(p.r)}</span>` : ''}
          ${p.c ? `<span class="flab" style="background:${LABELS[p.c]}"></span>` : ''}
        </div>`);
        c.onclick = (e) => {
          if (cmdKey(e)) { sel.has(i) ? sel.delete(i) : sel.add(i); render(); }
          else { sel = new Set(); go(i); }
        };
        strip.append(c);
      });
    }
    onKey(st, (e) => {
      if (e.metaKey || e.ctrlKey) return;
      if (e.key === 'ArrowRight') { go(cur + 1); return true; }
      if (e.key === 'ArrowLeft') { go(cur - 1); return true; }
      const targets = sel.size ? [...sel] : [cur];
      if (/^[0-5]$/.test(e.key)) { targets.forEach((t) => (P[t].r = +e.key)); render(); return true; }
      if (e.key === 'p' || e.key === 'P') { targets.forEach((t) => (P[t].f = 'pick')); render(); return true; }
      if (e.key === 'x' || e.key === 'X') { targets.forEach((t) => (P[t].f = 'reject')); render(); return true; }
      if (e.key === 'u' || e.key === 'U') { targets.forEach((t) => (P[t].f = null)); render(); return true; }
    });
    render();
    setTimeout(() => { tiffLoaded = true; render(); }, 4500);
    st.append(view, bar);
  };
})();
