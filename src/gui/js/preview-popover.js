// The picture a setting shows while its tile is hovered or one of its controls has focus: a block diorama
// of what the setting does (images/previews3d, made by scripts/previews3d). It has one fixed place, the
// foot of the settings sidebar above Reset, so it never covers the setting. Every setting shows its current
// state, a switch included; changing it shows the new one.

const BASE = 'images/previews3d/';
const W = 720, H = 540; // the pictures' pixel size; the page shows them small (settings.css)

// File name (no extension) for a setting in its current state, or for `button`, a choice button the
// pointer is on. `el` carries data-preview-on/-off (a switch), data-preview-num + data-preview-steps
// (nearest step), data-preview-seg (+ data-preview-key: the dataset key of a button's value, default "v")
// or data-preview-sel (a <select>). Null when the setting has no picture for that state.
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

export const captionKey = (name) => `preview_${name.replace(/[^a-z0-9]+/gi, '_')}`;

// The picture to show, as a list of one: the setting's current state, or a switch's other state where it
// has a picture for one only. A picture shared with another setting names its own caption in
// data-preview-on-caption / -off-caption.
export function previewFrames(el, button) {
  const d = el.dataset;
  if (button || d.previewSeg || d.previewSel || d.previewNum) {
    const name = previewName(el, button);
    return name ? [{ name, caption: captionKey(name) }] : [];
  }
  const frame = (name, key) => name && { name, caption: key || captionKey(name) };
  const off = frame(d.previewOff, d.previewOffCaption);
  const on = frame(d.previewOn, d.previewOnCaption);
  if (!off || !on || off.name === on.name) return [on || off].filter(Boolean);
  const row = el.closest('.settings-row');
  const sw = row && row.querySelector('input.switch');
  return [sw && sw.checked ? on : off];
}

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
  const sidebar = document.querySelector('.settings-sidebar');
  (sidebar || document.body).appendChild(pop);

  let open = false;
  let current = null; // the element the picture is for
  let want = null; // the file being loaded
  const hide = () => {
    open = false;
    want = null;
    if (current) current.removeAttribute('aria-describedby');
    pop.classList.remove('is-visible');
    pop.setAttribute('aria-hidden', 'true');
  };
  // Just above Reset, whatever its height is in this language.
  const place = () => {
    const reset = document.getElementById('reset-settings-button');
    if (!sidebar || !reset) return;
    pop.style.bottom = `${Math.round(sidebar.getBoundingClientRect().bottom - reset.getBoundingClientRect().top + 10)}px`;
  };
  // It appears once the picture is in, so the one before it never flashes; the caption is the text
  // (the alt is empty so a screen reader does not say it twice).
  const load = (frame) => {
    want = frame.name;
    img.onerror = hide;
    img.onload = () => {
      if (want !== frame.name) return;
      caption.textContent = (window.localization && window.localization[frame.caption]) || '';
      place();
      pop.classList.add('is-visible');
      pop.setAttribute('aria-hidden', 'false');
    };
    img.alt = '';
    img.src = `${BASE}${frame.name}.webp`;
  };
  const show = (el, button) => {
    const frames = previewFrames(el, button);
    if (!frames.length) return hide();
    if (current && current !== el) current.removeAttribute('aria-describedby');
    current = el;
    el.setAttribute('aria-describedby', pop.id);
    open = true;
    if (!pop.classList.contains('is-visible')) img.removeAttribute('src');
    load(frames[0]);
  };

  document.querySelectorAll('[data-preview-on], [data-preview-seg], [data-preview-sel], [data-preview-num]').forEach((el) => {
    const row = el.closest('.settings-row');
    if (!row) return;
    const seg = (n) => (el.dataset.previewSeg && n.classList && n.classList.contains('segment') ? n : null);
    row.addEventListener('mouseenter', () => show(el, null));
    row.addEventListener('mouseleave', hide);
    row.querySelectorAll('input, select, button, .tooltip-icon').forEach((n) => {
      n.addEventListener('focus', () => show(el, seg(n)));
      n.addEventListener('blur', hide);
      // A choice button shows its own picture; leaving it goes back to the chosen one.
      if (seg(n)) {
        n.addEventListener('mouseenter', () => show(el, n));
        n.addEventListener('mouseleave', () => show(el, null));
      }
    });
    const refresh = () => open && current === el && show(el, null);
    for (const n of row.querySelectorAll('input, select')) {
      n.addEventListener('change', refresh);
      n.addEventListener('input', refresh);
    }
  });

  window.addEventListener('resize', hide);
}
