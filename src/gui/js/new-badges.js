// "New!" on what came with this release: a badge (data-new, its key) shows on the first three starts
// and goes for good once data-new-when matches. A sidebar section carries one while it holds one.
export function initNewBadges(doc = document, store = localStorage) {
  const all = Array.from(doc.querySelectorAll('.new-badge[data-new]'));
  if (!all.length) return;
  const template = all[0].cloneNode();
  const read = (key) => {
    try { return { starts: 0, used: false, ...JSON.parse(store.getItem(`arnis-new-${key}`) || '{}') }; } catch (_) { return { starts: 0, used: false }; }
  };
  const write = (key, s) => { try { store.setItem(`arnis-new-${key}`, JSON.stringify(s)); } catch (_) { /* shown again */ } };
  // A setting row hides its unused revert button while a badge sits in its heading (settings.css).
  const row = (b) => b.closest('.setting-heading')?.closest('.settings-row');
  const drop = (key) => doc.querySelectorAll(`.new-badge[data-new="${key}"]`).forEach((b) => {
    row(b)?.classList.remove('has-new-badge');
    b.remove();
  });
  const markSections = () => {
    doc.querySelectorAll('.settings-nav-item').forEach((item) => {
      const up = !!doc.getElementById(item.dataset.target)?.querySelector('.new-badge[data-new]');
      const mark = item.querySelector('.new-badge');
      if (up && !mark) {
        const b = template.cloneNode();
        delete b.dataset.new;
        delete b.dataset.newWhen;
        item.insertBefore(b, item.querySelector('.settings-nav-dot'));
      } else if (!up && mark) mark.remove();
    });
  };
  const open = new Map();
  for (const key of new Set(all.map((b) => b.dataset.new))) {
    const when = all.find((b) => b.dataset.new === key && b.dataset.newWhen)?.dataset.newWhen || '';
    const seen = read(key);
    if (when && doc.querySelector(when)) seen.used = true;
    if (seen.used || seen.starts >= 3) {
      write(key, seen);
      drop(key);
      continue;
    }
    seen.starts += 1;
    write(key, seen);
    all.filter((b) => b.dataset.new === key).forEach((b) => row(b)?.classList.add('has-new-badge'));
    if (when) open.set(key, when);
  }
  markSections();
  if (!open.size) return;
  const EVENTS = ['change', 'click'];
  const later = () => setTimeout(check, 0);
  const check = () => {
    for (const [key, when] of open) {
      if (!doc.querySelector(when)) continue;
      write(key, { ...read(key), used: true });
      drop(key);
      open.delete(key);
    }
    markSections();
    if (!open.size) for (const ev of EVENTS) doc.removeEventListener(ev, later);
  };
  for (const ev of EVENTS) doc.addEventListener(ev, later);
}
