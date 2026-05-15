// Markdown subset renderer + platform-asset picker for the update modal.

function escapeHTML(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

const escapeAttr = escapeHTML;

// Markdown link/image URLs must use http(s); other schemes are dropped at render time.
function isSafeUrl(url) {
  return /^https?:\/\//i.test(url);
}

function sanitizeImgTag(attrsStr) {
  const out = {};
  const attrPattern = /(\w+)\s*=\s*(?:"([^"]*)"|'([^']*)'|(\S+))/g;
  let m;
  while ((m = attrPattern.exec(attrsStr)) !== null) {
    const key = m[1].toLowerCase();
    const val = m[2] !== undefined ? m[2] : (m[3] !== undefined ? m[3] : m[4]);
    if (["src", "alt", "title", "width", "height"].includes(key)) {
      out[key] = val;
    }
  }
  if (!out.src || !/^https?:\/\//i.test(out.src)) return "";
  const parts = [`<img src="${escapeAttr(out.src)}"`];
  if (out.alt !== undefined) parts.push(` alt="${escapeAttr(out.alt)}"`);
  if (out.title) parts.push(` title="${escapeAttr(out.title)}"`);
  if (out.width) parts.push(` width="${escapeAttr(out.width)}"`);
  if (out.height) parts.push(` height="${escapeAttr(out.height)}"`);
  parts.push(">");
  return parts.join("");
}

function sanitizeHtmlTag(raw) {
  if (/^<br\s*\/?>$/i.test(raw)) return "<br>";
  const img = raw.match(/^<img\s+([\s\S]*?)\/?>$/i);
  if (img) return sanitizeImgTag(img[1]);
  const open = raw.match(/^<(sub|sup|kbd)(?:\s[^>]*)?>$/i);
  if (open) return `<${open[1].toLowerCase()}>`;
  const close = raw.match(/^<\/(sub|sup|kbd)>$/i);
  if (close) return `</${close[1].toLowerCase()}>`;
  return "";
}

/// Strip Arnis-specific boilerplate that appears in every release body:
/// the "## Assets / * Windows / * Linux / * MacOS / Scroll down..." block,
/// the outer "## What's Changed" header that duplicates the one inside the
/// `<details>` wrapper, and the `<details>` collapsible itself (we render
/// the changelog inline instead of as a fold).
function stripBoilerplate(src) {
  src = src.replace(
    /^##?\s+Assets\s*\n[\s\S]*?Scroll down to find the download[^\n]*\n?/m,
    ""
  );
  src = src.replace(/^##\s+What['’]s Changed\s*\n+(?=<details)/im, "");
  src = src.replace(/<summary>[\s\S]*?<\/summary>/gi, "");
  src = src.replace(/<\/?details(?:\s[^>]*)?>/gi, "");
  return src;
}

/// Subset Markdown -> HTML. Pipeline: extract fenced code, then inline code,
/// then whitelisted HTML tags (img/details/summary/br/sub/sup/kbd) so each
/// can be re-emitted safely after escaping the rest of the source.
export function renderMarkdown(src) {
  if (!src) return "";

  src = stripBoilerplate(src);

  const codeBlocks = [];
  const inlineCodes = [];
  const sanitizedHtml = [];

  src = src.replace(/```([\w-]*)\n([\s\S]*?)```/g, (_, lang, code) => {
    const idx = codeBlocks.length;
    codeBlocks.push({ lang, code });
    return `\x00CB${idx}\x00`;
  });

  // Inline code extracted BEFORE HTML so `<details>` inside backticks stays literal.
  src = src.replace(/`([^`\n]+)`/g, (_, code) => {
    const idx = inlineCodes.length;
    inlineCodes.push(code);
    return `\x00IC${idx}\x00`;
  });

  const stashHtml = (raw) => {
    const sanitized = sanitizeHtmlTag(raw);
    if (!sanitized) return "";
    const idx = sanitizedHtml.length;
    sanitizedHtml.push(sanitized);
    return `\x00HT${idx}\x00`;
  };
  src = src.replace(/<img\s+[\s\S]*?\/?>/gi, stashHtml);
  src = src.replace(/<\/?(?:sub|sup|kbd)(?:\s[^>]*)?>/gi, stashHtml);
  src = src.replace(/<br\s*\/?>/gi, stashHtml);

  src = escapeHTML(src);

  // Capture groups here are already HTML-escaped (escapeHTML ran upstream);
  // re-escaping would turn `&amp;` into `&amp;amp;` and break URLs containing `&`.
  src = src.replace(
    /!\[([^\]]*)\]\(([^)\s]+)(?:\s+&quot;([^"]*)&quot;)?\)/g,
    (_, alt, url, title) => {
      if (!isSafeUrl(url)) return alt;
      const t = title ? ` title="${title}"` : "";
      return `<img src="${url}" alt="${alt}"${t}>`;
    }
  );
  src = src.replace(
    /\[([^\]]+)\]\(([^)\s]+)(?:\s+&quot;([^"]*)&quot;)?\)/g,
    (_, text, url, title) => {
      if (!isSafeUrl(url)) return text;
      const t = title ? ` title="${title}"` : "";
      return `<a href="${url}" target="_blank" rel="noopener noreferrer"${t}>${text}</a>`;
    }
  );
  src = src.replace(/\*\*([^\*\n][^\*\n]*?)\*\*/g, "<strong>$1</strong>");
  src = src.replace(/__([^_\n][^_\n]*?)__/g, "<strong>$1</strong>");
  src = src.replace(/(^|[^\*])\*([^\s\*][^\*\n]*?[^\s\*]|[^\s\*])\*(?!\*)/g, "$1<em>$2</em>");
  src = src.replace(/(^|[^_\w])_([^\s_][^_\n]*?[^\s_]|[^\s_])_(?!_)/g, "$1<em>$2</em>");

  const lines = src.split("\n");
  const out = [];
  let inList = null;
  let inBlockquote = false;
  let paraBuf = [];

  const flushPara = () => {
    if (paraBuf.length) { out.push(`<p>${paraBuf.join(" ")}</p>`); paraBuf = []; }
  };
  const flushList = () => {
    if (inList) { out.push(`</${inList}>`); inList = null; }
  };
  const flushBQ = () => {
    if (inBlockquote) { out.push("</blockquote>"); inBlockquote = false; }
  };

  for (const rawLine of lines) {
    const line = rawLine.replace(/\s+$/, "");
    if (!line.trim()) { flushPara(); flushList(); flushBQ(); continue; }

    if (/^(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line.trim())) {
      flushPara(); flushList(); flushBQ();
      out.push("<hr>");
      continue;
    }

    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      flushPara(); flushList(); flushBQ();
      out.push(`<h${h[1].length}>${h[2]}</h${h[1].length}>`);
      continue;
    }

    const ul = line.match(/^\s*[-*+]\s+(.*)$/);
    if (ul) {
      flushPara(); flushBQ();
      if (inList !== "ul") { flushList(); out.push("<ul>"); inList = "ul"; }
      out.push(`<li>${ul[1]}</li>`);
      continue;
    }

    const ol = line.match(/^\s*\d+\.\s+(.*)$/);
    if (ol) {
      flushPara(); flushBQ();
      if (inList !== "ol") { flushList(); out.push("<ol>"); inList = "ol"; }
      out.push(`<li>${ol[1]}</li>`);
      continue;
    }

    const bq = line.match(/^&gt;\s?(.*)$/);
    if (bq) {
      flushPara(); flushList();
      if (!inBlockquote) { out.push("<blockquote>"); inBlockquote = true; }
      out.push(`${bq[1]}<br>`);
      continue;
    }

    if (/^\x00CB\d+\x00$/.test(line.trim())) {
      flushPara(); flushList(); flushBQ();
      out.push(line.trim());
      continue;
    }

    flushList(); flushBQ();
    paraBuf.push(line);
  }
  flushPara(); flushList(); flushBQ();

  let html = out.join("\n");
  html = html.replace(/\x00IC(\d+)\x00/g, (_, idx) =>
    `<code>${escapeHTML(inlineCodes[+idx])}</code>`);
  html = html.replace(/\x00CB(\d+)\x00/g, (_, idx) => {
    const { lang, code } = codeBlocks[+idx];
    const cls = lang ? ` class="language-${escapeAttr(lang)}"` : "";
    return `<pre><code${cls}>${escapeHTML(code)}</code></pre>`;
  });
  html = html.replace(/\x00HT(\d+)\x00/g, (_, idx) => sanitizedHtml[+idx]);
  return html;
}

const PLATFORM_ASSET_PATTERNS = {
  windows: [/windows.*\.exe$/i, /windows/i, /\.exe$/i],
  macos: [/mac.*universal/i, /macos/i, /mac/i, /darwin/i],
  linux: [/linux/i],
};

export function pickAssetForPlatform(assets, platform) {
  if (!assets || !assets.length) return null;
  const patterns = PLATFORM_ASSET_PATTERNS[platform];
  if (!patterns) return null;
  for (const pat of patterns) {
    const hit = assets.find((a) => pat.test(a.name));
    if (hit) return hit;
  }
  return null;
}
