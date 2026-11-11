// The picture that appears when the info icon of a setting, or one of its choice buttons, is
// hovered or focused: an isometric block diorama of what the setting does (src/gui/images/previews3d,
// made by scripts/previews3d), small and beside the control, never over it. Only settings that carry data-preview-on/-off (a
// switch), data-preview-seg (a segmented control, one picture per button value) or data-preview-sel (a
// select, one per option) get one.

const BASE = 'images/previews3d/';
const W = 720, H = 540; // the pictures' pixel size; the page shows them small (settings.css)

// File name (no extension) for a setting in its current state, or for `button`, a choice button the
// pointer is on. `el` is the setting's ? icon or its row and carries data-preview-on/-off (a switch),
// data-preview-num + data-preview-steps (a slider or number box, nearest step), data-preview-seg (+ optional data-preview-key: the dataset key that holds a button's value, default
// "v") or data-preview-sel (a <select>). Null when the setting has no picture for that state.
export function previewName(el, button) {
  const row = el.closest('.settings-row');
  if (!row) return null;
  if (el.dataset.previewSeg) {
    const b = button || row.querySelector('.segment.active');
    const v = b && b.dataset.nopreview === undefined && b.dataset[el.dataset.previewKey || 'v'];
    return v ? `${el.dataset.previewSeg}-${v}` : null;
  }
  if (el.dataset.previewSel) {
    const sel = row.querySelector('select');
    return sel && sel.value ? `${el.dataset.previewSel}-${sel.value}` : null;
  }
  if (el.dataset.previewNum) {
    // A slider or number box: the picture of the nearest of data-preview-steps ("0.5,1,2"), by size.
    const input = row.querySelector('input[type=range], input[type=number]');
    const v = input && Math.abs(parseFloat(input.value));
    if (!(v >= 0)) return null;
    const steps = el.dataset.previewSteps.split(',');
    const best = steps.reduce((a, c) => (Math.abs(c - v) < Math.abs(a - v) ? c : a));
    return `${el.dataset.previewNum}-${best}`;
  }
  const sw = row.querySelector('input.switch');
  return (sw && sw.checked ? el.dataset.previewOn : el.dataset.previewOff) || null;
}

const GAP = 12, MARGIN = 8;
const hits = (a, b) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;

// Where the popover (w x h) goes: top-left corner (and its width), in a vw x vh window. `obstacle` is the
// control (and its text tooltip) that must stay uncovered, `panel` the settings column, `avoid` the
// rects of text it must not cover. First room that fits wins: the free side of the panel (left in LTR,
// right in RTL), the other side, beside the control; failing that the nearest hole in the text, shrunk
// down to MIN_W wide (the picture's height follows, 4:3); last, above or below the control (no `fit`).
// Vertically it follows the control, clamped into the window.
const MIN_W = 220, FRAME = 22; // FRAME: the popover's padding + border around the picture
export function placePopover({ obstacle, panel, vw, vh, w, h, rtl, avoid = [] }) {
  const clear = (r) => !hits(r, obstacle) && !avoid.some((a) => hits(r, a));
  const top = Math.max(MARGIN, Math.min((obstacle.top + obstacle.bottom) / 2 - h / 2, vh - h - MARGIN));
  const lefts = (r) => [r.left - GAP - w, r.right + GAP]; // [before, after] a rect in LTR
  const order = [lefts(panel), lefts(obstacle)].flatMap(([a, b]) => (rtl ? [b, a] : [a, b]));
  for (const left of order) {
    const r = { left, right: left + w, top, bottom: top + h };
    if (left >= MARGIN && r.right <= vw - MARGIN && clear(r)) return { left, top, w, fit: true };
  }
  const cx = (obstacle.left + obstacle.right) / 2, cy = (obstacle.top + obstacle.bottom) / 2;
  for (let wi = w - 24; wi >= MIN_W - 24; wi -= 24) {
    const ww = Math.max(MIN_W, wi);
    const hh = h - (w - ww) * 0.75 + (ww < w ? 18 : 0); // the picture is 4:3; a narrower caption may take a line more
    let best = null;
    for (let l = MARGIN; l + ww <= vw - MARGIN; l += 8) {
      for (let t = MARGIN; t + hh <= vh - MARGIN; t += 8) {
        const r = { left: l, right: l + ww, top: t, bottom: t + hh };
        if (!clear(r)) continue;
        const d = (l + ww / 2 - cx) ** 2 + (t + hh / 2 - cy) ** 2;
        if (!best || d < best.d) best = { left: l, top: t, w: ww, d };
      }
    }
    if (best) return { left: best.left, top: best.top, w: best.w, fit: true };
  }
  const left = Math.max(MARGIN, Math.min(cx - w / 2, vw - w - MARGIN));
  const above = obstacle.top - GAP - h;
  return { left, top: above >= MARGIN ? above : Math.min(obstacle.bottom + GAP, vh - h - MARGIN), w };
}

// The rects of the text inside `root` that is on screen (one per line), tight to the glyphs.
function textRects(root, vw, vh) {
  if (!root || !document.createTreeWalker) return [];
  const out = [];
  const range = document.createRange();
  for (const walker = document.createTreeWalker(root, 4); walker.nextNode();) {
    if (!walker.currentNode.nodeValue.trim()) continue;
    range.selectNodeContents(walker.currentNode);
    for (const r of range.getClientRects()) if (r.width && r.bottom > 0 && r.top < vh && r.right > 0 && r.left < vw) out.push(r);
  }
  return out;
}

export const captionKey = (name) => `preview_${name.replace(/[^a-z0-9]+/gi, '_')}`;

export function initPreviews() {
  const pop = document.createElement('div');
  pop.className = 'preview-popover';
  pop.id = 'preview-popover';
  pop.setAttribute('role', 'tooltip');
  pop.setAttribute('aria-hidden', 'true');
  const img = document.createElement('img');
  img.decoding = 'async';
  img.width = W;
  img.height = H;
  const caption = document.createElement('div');
  caption.className = 'preview-popover-caption';
  pop.appendChild(img);
  pop.appendChild(caption);
  document.body.appendChild(pop);

  let open = false;
  let current = null; // the icon the picture is for
  let want = null; // the file being loaded
  const hide = () => {
    open = false;
    want = null;
    if (current) current.removeAttribute('aria-describedby');
    pop.classList.remove('is-visible');
    pop.setAttribute('aria-hidden', 'true');
  };
  let ctl = null; // the control the pointer or focus is on
  // Beside the control, and clear of its text tooltip when one is up.
  const place = () => {
    const c = ctl.getBoundingClientRect();
    const t = document.querySelector('.global-tooltip.is-visible');
    const tr = t && t.getBoundingClientRect();
    const obstacle = tr ? { left: Math.min(c.left, tr.left), right: Math.max(c.right, tr.right), top: Math.min(c.top, tr.top), bottom: Math.max(c.bottom, tr.bottom) } : c;
    const panel = (document.querySelector('.settings-content') || ctl).getBoundingClientRect();
    img.style.width = ''; // full size first
    const p = pop.getBoundingClientRect();
    const vw = window.innerWidth, vh = window.innerHeight;
    const args = { obstacle, panel, vw, vh, w: p.width, h: p.height, rtl: document.documentElement && document.documentElement.dir === 'rtl' };
    const rows = textRects(document.querySelector('.settings-content'), vw, vh);
    // Clear of the sidebar's text too when there is room; else over the sidebar, never over a setting's text.
    let at = placePopover({ ...args, avoid: rows.concat(textRects(document.querySelector('.settings-sidebar'), vw, vh)) });
    if (!at.fit) at = placePopover({ ...args, avoid: rows });
    if (at.w < p.width) img.style.width = `${at.w - FRAME}px`;
    pop.style.left = `${at.left}px`;
    pop.style.top = `${at.top}px`;
  };
  const show = (icon, button, control) => {
    if (control) ctl = control;
    const name = previewName(icon, button);
    if (!name) return hide();
    const text = (window.localization && window.localization[captionKey(name)]) || '';
    if (current && current !== icon) current.removeAttribute('aria-describedby');
    current = icon;
    icon.setAttribute('aria-describedby', pop.id);
    // The popover appears once the picture is in, so the one before it never flashes; the caption is the
    // text (the picture's alt is empty so a screen reader does not say it twice).
    pop.classList.remove('is-visible');
    img.onerror = hide; // a picture that is missing leaves the icon's text tooltip alone
    want = name;
    img.onload = () => {
      if (want === name) {
        place();
        pop.classList.add('is-visible');
        pop.setAttribute('aria-hidden', 'false');
      }
    };
    img.removeAttribute('src');
    img.alt = '';
    img.src = `${BASE}${name}.webp`;
    caption.textContent = text;
    open = true;
  };

  // Every control of the setting shows it: the ? icon, the switch, the select, each choice button.
  const on = (n, f) => {
    for (const ev of ['mouseenter', 'focus']) n.addEventListener(ev, () => f(n));
    for (const ev of ['mouseleave', 'blur']) n.addEventListener(ev, hide);
  };
  document.querySelectorAll('[data-preview-on], [data-preview-seg], [data-preview-sel], [data-preview-num]').forEach((icon) => {
    const row = icon.closest('.settings-row');
    if (icon !== row) on(icon, (n) => show(icon, null, n));
    const sw = row.querySelector('input.switch');
    if (sw) on(sw, (n) => show(icon, null, n));
    const sel = row.querySelector('select');
    if (sel && icon.dataset.previewSel) {
      on(sel, (n) => show(icon, null, n));
      sel.addEventListener('change', () => open && show(icon, null));
    }
    const num = row.querySelector('input[type=range], input[type=number]');
    if (num && icon.dataset.previewNum) {
      on(num, (n) => show(icon, null, n));
      num.addEventListener('input', () => open && show(icon, null));
    }
    if (icon.dataset.previewSeg) row.querySelectorAll('.segment').forEach((b) => on(b, (n) => show(icon, b, n)));
  });

  // Capture phase: this runs before the document handler that closes the settings page, so Escape
  // closes the picture first and the page on the next press.
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && open) {
      const icon = current;
      hide();
      // Louis's text tooltip hides on mouseleave; it would otherwise stay up with the picture gone.
      if (icon) icon.dispatchEvent(new Event('mouseleave'));
      e.stopPropagation();
    }
  }, true);
  window.addEventListener('resize', hide);
  const scrollable = document.querySelector('.settings-scrollable');
  if (scrollable) scrollable.addEventListener('scroll', hide, { passive: true });
}
