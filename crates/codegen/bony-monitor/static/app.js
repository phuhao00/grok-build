const state = {
  changes: [],
  features: null,
  workflow: null,
  tag: "all",
  search: "",
  filterMode: "feature", // feature | area
  page: "overview",
  codeMapLayer: "all",
  codeMapSearch: "",
};

const PAGE_META = {
  overview: {
    crumb: "Monitor / Overview",
    title: "Overview",
    sub: "Feature heat map and system layers",
  },
  workflow: {
    crumb: "Monitor / How it works",
    title: "How it works",
    sub: "Charts + source map from UI → ACP → Session → tools",
  },
  changes: {
    crumb: "Monitor / Changelog",
    title: "Changelog",
    sub: "Recent commits with product impact",
  },
};

const AREA_LABELS = {
  all: "All",
  desktop: "Desktop",
  monitor: "Monitor",
  "agent-runtime": "Runtime",
  tui: "TUI",
  docs: "Docs",
  workspace: "Workspace",
  config: "Config",
  other: "Other",
};

async function fetchJson(url) {
  const res = await fetch(url);
  if (!res.ok) throw new Error(await res.text());
  return res.json();
}

function fmtNum(n) {
  return new Intl.NumberFormat("zh-CN").format(n ?? 0);
}

function escapeHtml(s) {
  return String(s ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function setPage(page) {
  state.page = page;
  document.querySelectorAll(".nav-item").forEach((btn) => {
    const on = btn.dataset.page === page;
    btn.classList.toggle("active", on);
    if (on) btn.setAttribute("aria-current", "page");
    else btn.removeAttribute("aria-current");
  });
  document.querySelectorAll(".page").forEach((el) => {
    const show = el.id === `page-${page}`;
    el.classList.toggle("hidden", !show);
    if (show) {
      el.style.animation = "none";
      void el.offsetWidth;
      el.style.animation = "";
    }
  });
  const meta = PAGE_META[page] || PAGE_META.overview;
  const crumb = document.getElementById("crumb");
  const title = document.getElementById("page-title");
  const sub = document.getElementById("page-sub");
  if (crumb) crumb.textContent = meta.crumb;
  if (title) title.textContent = meta.title;
  if (sub) sub.textContent = meta.sub;

  const stats = document.getElementById("stats");
  if (stats) stats.classList.toggle("hidden-stats", page !== "overview");

  if (location.hash.replace("#", "") !== page) {
    history.replaceState(null, "", `#${page}`);
  }
  renderFilters();
  if (page === "changes") {
    renderTimeline();
  }
  document.querySelector(".main-scroll")?.scrollTo({ top: 0, behavior: "smooth" });
  window.scrollTo({ top: 0, behavior: "smooth" });
}

function renderStats(overview) {
  const el = document.getElementById("stats");
  el.innerHTML = [
    ["Commits", overview.commit_count],
    ["Additions", overview.additions],
    ["Deletions", overview.deletions],
    ["Features hit", overview.features_touched],
    ["Catalog", overview.features_total],
  ]
    .map(
      ([label, value]) => `
      <div class="stat">
        <div class="label">${escapeHtml(label)}</div>
        <div class="value">${fmtNum(value)}</div>
      </div>`
    )
    .join("");

  const repo = overview.repo || "";
  const shortRepo = repo.split(/[/\\]/).slice(-2).join("/") || repo;
  document.getElementById("top-meta").innerHTML = `
    <div title="${escapeHtml(repo)}">${escapeHtml(shortRepo)}</div>
    <div>HEAD ${escapeHtml(overview.latest?.short_sha ?? "-")}</div>
  `;
}

function renderFeatureMatrix(features) {
  const el = document.getElementById("feature-grid");
  el.innerHTML = (features.activity || [])
    .map((a) => {
      const f = a.feature;
      const last = a.commit_count
        ? `${escapeHtml(a.last_sha)} · ${escapeHtml(a.last_subject)}`
        : "No recent activity";
      return `
        <article class="feat ${escapeHtml(a.heat)}" data-feature="${escapeHtml(
          f.id
        )}" tabindex="0" role="button">
          <div class="feat-top">
            <h3>${escapeHtml(f.name)}</h3>
            <span class="heat">${escapeHtml(a.heat)} · ${a.commit_count} commits</span>
          </div>
          <div class="cat">${escapeHtml(f.category)}${f.user_facing ? " · user-facing" : " · internal"}</div>
          <p>${escapeHtml(f.description)}</p>
          <div class="feat-meta">
            <span class="add">+${a.additions}</span>
            <span class="del">-${a.deletions}</span>
            · ${last}
          </div>
        </article>`;
    })
    .join("");
}

function goFeatureFilter(featureId) {
  state.tag = featureId || "all";
  state.filterMode = "feature";
  state.search = "";
  const search = document.getElementById("change-search");
  if (search) search.value = "";
  setPage("changes");
  requestAnimationFrame(() => {
    document.getElementById("timeline")?.scrollIntoView({ behavior: "smooth", block: "start" });
  });
}

function renderArchitecture(arch) {
  document.getElementById("arch-blurb").textContent = arch.blurb;
  document.getElementById("layers").innerHTML = arch.layers
    .map(
      (layer) => `
      <article class="layer">
        <h3>${escapeHtml(layer.name)}</h3>
        <p>${escapeHtml(layer.summary)}</p>
        <div class="crates">
          ${layer.crates.map((c) => `<span class="chip">${escapeHtml(c)}</span>`).join("")}
        </div>
      </article>`
    )
    .join("");

  document.getElementById("flows").innerHTML = `
    <ol class="flows">
      ${arch.flows.map((f) => `<li>${escapeHtml(f)}</li>`).join("")}
    </ol>`;

  document.getElementById("diagrams").innerHTML = arch.diagrams
    .map(
      (d) => `
      <figure class="diagram">
        <div class="cap">${escapeHtml(d.title)}</div>
        <img src="${escapeHtml(d.path)}" alt="${escapeHtml(d.title)}" loading="lazy" />
      </figure>`
    )
    .join("");
}

function openLightbox(src, caption) {
  const box = document.getElementById("lightbox");
  document.getElementById("lightbox-img").src = src;
  document.getElementById("lightbox-img").alt = caption || "";
  document.getElementById("lightbox-cap").textContent = caption || "";
  box.classList.remove("hidden");
}

function closeLightbox() {
  document.getElementById("lightbox").classList.add("hidden");
  document.getElementById("lightbox-img").src = "";
}

function openScene(id, { scroll = true } = {}) {
  document.querySelectorAll(".scene").forEach((el) => {
    const on = el.dataset.scene === id;
    el.classList.toggle("open", on);
    const btn = el.querySelector(".scene-toggle");
    if (btn) btn.setAttribute("aria-expanded", on ? "true" : "false");
  });
  if (scroll) {
    const el = document.querySelector(`.scene[data-scene="${CSS.escape(id)}"]`);
    el?.scrollIntoView({ behavior: "smooth", block: "start" });
  }
}

function highlightStep(sceneId, stepN) {
  openScene(sceneId, { scroll: true });
  document.querySelectorAll(".pipe-node").forEach((n) => {
    n.classList.toggle("active", Number(n.dataset.step) === stepN && n.dataset.scene === sceneId);
  });
  requestAnimationFrame(() => {
    const step = document.querySelector(
      `.scene[data-scene="${CSS.escape(sceneId)}"] .wf-step[data-step="${stepN}"]`
    );
    if (!step) return;
    step.classList.remove("pulse");
    void step.offsetWidth;
    step.classList.add("pulse");
    step.scrollIntoView({ behavior: "smooth", block: "center" });
  });
}

function chartById(id) {
  return (state.workflow?.charts || []).find((c) => c.id === id);
}

function renderChartCard(chart) {
  if (!chart) return "";
  return `
    <article class="chart-card" data-chart="${escapeHtml(chart.id)}">
      <header>
        <h3>${escapeHtml(chart.title)}</h3>
        <p class="muted">${escapeHtml(chart.caption || "")}</p>
      </header>
      <div class="chart-body">${renderChartSvg(chart)}</div>
    </article>`;
}

function renderChartSvg(chart) {
  switch (chart.kind) {
    case "sequence":
      return svgSequence(chart);
    case "layers":
      return svgLayers(chart);
    case "flow":
    case "callgraph":
      return svgGraph(chart);
    case "bars":
      return svgBars(chart);
    default:
      return `<p class="muted">Unknown chart kind</p>`;
  }
}

function svgSequence(chart) {
  const actors = chart.actors || [];
  const messages = chart.messages || [];
  const markerId = `arrow-${String(chart.id || "seq").replace(/[^a-z0-9_-]/gi, "")}`;
  const colW = 118;
  const left = 28;
  const top = 42;
  const rowH = 34;
  const width = Math.max(640, left * 2 + actors.length * colW);
  const height = top + 28 + messages.length * rowH + 24;
  const xOf = (name) => {
    const i = actors.indexOf(name);
    return left + (i < 0 ? 0 : i) * colW + colW / 2;
  };

  const heads = actors
    .map((a, i) => {
      const x = left + i * colW + colW / 2;
      return `
        <rect x="${x - 48}" y="8" width="96" height="26" rx="8" class="seq-head" />
        <text x="${x}" y="25" text-anchor="middle" class="seq-head-text">${escapeHtml(a)}</text>
        <line x1="${x}" y1="${top}" x2="${x}" y2="${height - 12}" class="seq-life" />`;
    })
    .join("");

  const lines = messages
    .map((m, i) => {
      const y = top + 18 + i * rowH;
      const x1 = xOf(m.from);
      const x2 = xOf(m.to);
      const mid = (x1 + x2) / 2;
      const dir = x2 >= x1 ? 1 : -1;
      return `
        <line x1="${x1}" y1="${y}" x2="${x2 - 8 * dir}" y2="${y}" class="seq-msg" marker-end="url(#${markerId})" />
        <text x="${mid}" y="${y - 6}" text-anchor="middle" class="seq-label">${escapeHtml(
          m.label
        )}</text>`;
    })
    .join("");

  return `
    <svg class="chart-svg sequence" viewBox="0 0 ${width} ${height}" role="img" aria-label="${escapeHtml(
      chart.title
    )}">
      <defs>
        <marker id="${markerId}" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto">
          <path d="M0,0 L6,3 L0,6 Z" fill="#0a84ff" />
        </marker>
      </defs>
      ${heads}${lines}
    </svg>`;
}

function svgLayers(chart) {
  const bands = chart.bands || [];
  const width = 720;
  const rowH = 72;
  const height = 16 + bands.length * (rowH + 10);
  const rows = bands
    .map((b, i) => {
      const y = 12 + i * (rowH + 10);
      const chips = (b.items || [])
        .map(
          (it, j) =>
            `<span class="layer-chip" style="--i:${j}">${escapeHtml(it)}</span>`
        )
        .join("");
      return `
        <div class="layer-band" style="--y:${y}px">
          <div class="layer-band-title">
            <strong>${escapeHtml(b.name)}</strong>
            <span class="muted">${escapeHtml(b.summary || "")}</span>
          </div>
          <div class="layer-band-items">${chips}</div>
        </div>`;
    })
    .join("");
  return `<div class="layer-stack" style="min-height:${height}px">${rows}</div>`;
}

function svgGraph(chart) {
  const nodes = chart.nodes || [];
  const edges = chart.edges || [];
  const markerId = `garrow-${String(chart.id || "g").replace(/[^a-z0-9_-]/gi, "")}`;
  const cols = Math.min(4, Math.max(2, Math.ceil(Math.sqrt(nodes.length))));
  const cellW = 168;
  const cellH = 88;
  const width = 40 + cols * cellW;
  const rows = Math.ceil(nodes.length / cols) || 1;
  const height = 40 + rows * cellH;
  const pos = {};
  nodes.forEach((n, i) => {
    const c = i % cols;
    const r = Math.floor(i / cols);
    pos[n.id] = { x: 28 + c * cellW + cellW / 2, y: 28 + r * cellH + 28 };
  });

  const edgeLines = edges
    .map((e) => {
      const a = pos[e.from];
      const b = pos[e.to];
      if (!a || !b) return "";
      const mx = (a.x + b.x) / 2;
      const my = (a.y + b.y) / 2;
      return `
        <line x1="${a.x}" y1="${a.y}" x2="${b.x}" y2="${b.y}" class="graph-edge" marker-end="url(#${markerId})" />
        <text x="${mx}" y="${my - 4}" text-anchor="middle" class="graph-edge-label">${escapeHtml(
          e.label || ""
        )}</text>`;
    })
    .join("");

  const nodeEls = nodes
    .map((n) => {
      const p = pos[n.id];
      return `
        <g class="graph-node">
          <rect x="${p.x - 70}" y="${p.y - 22}" width="140" height="44" rx="10" />
          <text x="${p.x}" y="${p.y - 2}" text-anchor="middle" class="graph-title">${escapeHtml(
            n.label
          )}</text>
          <text x="${p.x}" y="${p.y + 14}" text-anchor="middle" class="graph-detail">${escapeHtml(
            n.detail || ""
          )}</text>
        </g>`;
    })
    .join("");

  return `
    <svg class="chart-svg graph" viewBox="0 0 ${width} ${height}" role="img" aria-label="${escapeHtml(
      chart.title
    )}">
      <defs>
        <marker id="${markerId}" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto">
          <path d="M0,0 L6,3 L0,6 Z" fill="#8e8e93" />
        </marker>
      </defs>
      ${edgeLines}${nodeEls}
    </svg>`;
}

function svgBars(chart) {
  const bars = chart.bars || [];
  if (!bars.length) return `<p class="muted">No scanned modules yet</p>`;
  const max = Math.max(1, ...bars.map((b) => Number(b.value) || 0));
  const rows = bars
    .map((b) => {
      const pct = Math.round(((Number(b.value) || 0) / max) * 100);
      return `
        <div class="bar-row">
          <div class="bar-label" title="${escapeHtml(b.note || "")}">${escapeHtml(b.label)}</div>
          <div class="bar-track"><div class="bar-fill" style="width:${pct}%"></div></div>
          <div class="bar-value">${escapeHtml(String(b.value))}</div>
        </div>`;
    })
    .join("");
  return `<div class="bar-chart">${rows}</div>`;
}

function renderCodeRefs(refs) {
  if (!refs?.length) return "";
  return `
    <div class="code-refs">
      ${refs
        .map(
          (r) => `
        <div class="code-ref">
          <code class="code-path">${escapeHtml(r.path)}</code>
          <span class="code-sym">${escapeHtml(r.symbol)}</span>
          <span class="muted">${escapeHtml(r.note || "")}</span>
        </div>`
        )
        .join("")}
    </div>`;
}

function renderCodeMap() {
  const wf = state.workflow;
  const tbody = document.querySelector("#workflow-code-map tbody");
  const layerSel = document.getElementById("code-map-layer");
  if (!tbody || !wf) return;

  const layers = ["all", ...new Set((wf.code_map || []).map((m) => m.layer).filter(Boolean))];
  if (layerSel && layerSel.options.length !== layers.length) {
    const prev = state.codeMapLayer;
    layerSel.innerHTML = layers
      .map(
        (l) =>
          `<option value="${escapeHtml(l)}"${l === prev ? " selected" : ""}>${escapeHtml(
            l === "all" ? "All layers" : l
          )}</option>`
      )
      .join("");
  }

  const q = (state.codeMapSearch || "").trim().toLowerCase();
  const rows = (wf.code_map || []).filter((m) => {
    if (state.codeMapLayer !== "all" && m.layer !== state.codeMapLayer) return false;
    if (!q) return true;
    const hay = [m.path, m.stem, m.crate_name, m.role, ...(m.key_types || [])]
      .join(" ")
      .toLowerCase();
    return hay.includes(q);
  });

  tbody.innerHTML = rows.length
    ? rows
        .map(
          (m) => `
      <tr>
        <td><span class="layer-tag">${escapeHtml(m.layer)}</span></td>
        <td>${escapeHtml(m.crate_name)}</td>
        <td><code>${escapeHtml(m.path)}</code></td>
        <td>${escapeHtml(m.role)}</td>
        <td class="syms">${(m.key_types || [])
          .map((t) => `<code>${escapeHtml(t)}</code>`)
          .join(" ")}</td>
      </tr>`
        )
        .join("")
    : `<tr><td colspan="5" class="muted">No modules match this filter.</td></tr>`;
}

function renderWorkflow(wf) {
  if (!wf) return;
  document.getElementById("workflow-question").textContent = wf.question;
  document.getElementById("workflow-answer").textContent = wf.answer;
  document.getElementById("workflow-blurb").textContent = wf.blurb;
  const meta = document.getElementById("workflow-module-meta");
  if (meta) {
    meta.textContent = `Live scan: ${wf.module_count ?? 0} source files · ${
      wf.desktop_module_count ?? 0
    } desktop modules · ${(wf.charts || []).length} charts`;
  }

  const chartRoot = document.getElementById("workflow-charts");
  if (chartRoot) {
    chartRoot.innerHTML = (wf.charts || []).map((c) => renderChartCard(c)).join("");
  }
  renderCodeMap();

  const firstScene = wf.scenes?.[0];
  const pipeline = document.getElementById("workflow-pipeline");
  const pipeSource =
    (wf.pipeline || []).length > 0
      ? wf.pipeline.map((label, i) => ({
          label,
          n: i + 1,
          scene: firstScene?.id || "ask",
        }))
      : (firstScene?.steps || []).map((s) => ({
          label: s.chip || s.title,
          n: s.n,
          scene: firstScene.id,
        }));
  pipeline.innerHTML = pipeSource
    .map((c, i) => {
      const node = `<button type="button" class="pipe-node" data-scene="${escapeHtml(
        c.scene
      )}" data-step="${c.n}"><span class="pn">${i + 1}</span>${escapeHtml(c.label)}</button>`;
      return i === 0 ? node : `<span class="pipe-arrow" aria-hidden="true">→</span>${node}`;
    })
    .join("");

  const openId =
    document.querySelector(".scene.open")?.dataset.scene || firstScene?.id || "";
  const scenes = document.getElementById("workflow-scenes");
  scenes.innerHTML = (wf.scenes || [])
    .map((scene, idx) => {
      const isOpen = openId ? scene.id === openId : idx === 0;
      const steps = (scene.steps || [])
        .map(
          (s) => `
        <article class="wf-step" data-step="${s.n}" id="step-${escapeHtml(scene.id)}-${s.n}">
          <div class="wf-n">${s.n}</div>
          <div class="wf-body">
            <h3>${escapeHtml(s.title)}</h3>
            <div class="wf-actor">${escapeHtml(s.actor)}</div>
            <p>${escapeHtml(s.action)}</p>
            <div class="wf-artifact"><span>Output</span> <code>${escapeHtml(s.artifact)}</code></div>
            <div class="crates">
              ${(s.crates || []).map((c) => `<span class="chip">${escapeHtml(c)}</span>`).join("")}
            </div>
            ${renderCodeRefs(s.code_refs)}
          </div>
        </article>`
        )
        .join("");

      const hasImage = Boolean(scene.image);
      const side = scene.image_side === "left" ? "img-left" : "img-right";
      const visual = hasImage
        ? `<figure class="scene-visual">
            <img src="${escapeHtml(scene.image)}" alt="${escapeHtml(
              scene.image_caption || scene.title
            )}" loading="lazy" />
            <figcaption>${escapeHtml(scene.image_caption || "")}</figcaption>
          </figure>`
        : `<div class="scene-placeholder">Charts above cover this path</div>`;

      const embedded = (scene.chart_ids || [])
        .map((id) => chartById(id))
        .filter(Boolean)
        .map((c) => renderChartCard(c))
        .join("");

      return `
        <section class="panel full scene ${isOpen ? "open" : ""}" data-scene="${escapeHtml(
          scene.id
        )}">
          <button type="button" class="scene-toggle" aria-expanded="${isOpen ? "true" : "false"}">
            <span>
              <h2>${escapeHtml(scene.title)}</h2>
              <span class="muted">${escapeHtml(scene.summary)}</span>
            </span>
            <span class="scene-chevron" aria-hidden="true">›</span>
          </button>
          <div class="scene-body">
            <div class="scene-grid ${hasImage ? side : "no-image"}">
              <div class="scene-copy">
                <div class="wf-steps">${steps}</div>
              </div>
              ${visual}
            </div>
            ${
              embedded
                ? `<div class="scene-charts chart-grid">${embedded}</div>`
                : ""
            }
          </div>
        </section>`;
    })
    .join("");

  document.getElementById("workflow-gallery").innerHTML = (wf.gallery || [])
    .map(
      (g) => `
      <article class="gallery-card" tabindex="0" role="button" aria-label="Enlarge ${escapeHtml(
        g.title
      )}">
        <img src="${escapeHtml(g.path)}" alt="${escapeHtml(g.title)}" loading="lazy" />
        <div class="cap">
          <h3>${escapeHtml(g.title)}</h3>
          <p>${escapeHtml(g.caption)}</p>
        </div>
      </article>`
    )
    .join("");
}

function formatWhen(iso) {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return String(iso).slice(0, 16);
  const now = new Date();
  const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const startThat = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const dayDiff = Math.round((startToday - startThat) / 86400000);
  const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  if (dayDiff === 0) return `Today · ${time}`;
  if (dayDiff === 1) return `Yesterday · ${time}`;
  if (dayDiff < 7) return `${d.toLocaleDateString(undefined, { weekday: "short" })} · ${time}`;
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" }) + ` · ${time}`;
}

function dayKey(iso) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return String(iso).slice(0, 10) || "Unknown";
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
}

function dayLabel(iso) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "Unknown date";
  const now = new Date();
  const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const startThat = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const dayDiff = Math.round((startToday - startThat) / 86400000);
  if (dayDiff === 0) return "Today";
  if (dayDiff === 1) return "Yesterday";
  return d.toLocaleDateString(undefined, {
    weekday: "long",
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

function filteredChanges() {
  const q = state.search.trim().toLowerCase();
  return state.changes.filter((c) => {
    const byFeature =
      state.tag === "all" ||
      (c.impact.features || []).some((f) => f.id === state.tag) ||
      (c.impact.tags || []).includes(state.tag);
    if (!byFeature) return false;
    if (!q) return true;
    const hay = [
      c.subject,
      c.author,
      c.short_sha,
      c.sha,
      ...(c.impact.features || []).map((f) => f.name),
    ]
      .join(" ")
      .toLowerCase();
    return hay.includes(q);
  });
}

function renderFilters() {
  const select = document.getElementById("filter-select");
  if (!select) return;

  const activity = state.features?.activity || [];
  const catalog = state.features?.catalog || [];
  const byId = new Map();

  for (const f of catalog) {
    byId.set(f.id, { id: f.id, name: f.name, commits: 0 });
  }
  // Activity is heat-sorted; overlay commit counts and prefer that order.
  const ordered = [];
  const seen = new Set();
  for (const a of activity) {
    const id = a.feature?.id;
    if (!id || seen.has(id)) continue;
    seen.add(id);
    ordered.push({
      id,
      name: a.feature.name || id,
      commits: a.commit_count || 0,
    });
  }
  for (const item of byId.values()) {
    if (!seen.has(item.id)) ordered.push(item);
  }

  const opts = [
    { id: "all", name: "All features", commits: state.changes.length },
    ...ordered,
  ];

  // Keep current filter if it still exists; otherwise reset to all.
  if (state.tag !== "all" && !opts.some((o) => o.id === state.tag)) {
    state.tag = "all";
  }

  const html = opts
    .map((o) => {
      const label =
        o.id === "all"
          ? `All features (${o.commits})`
          : o.commits > 0
            ? `${o.name} (${o.commits})`
            : o.name;
      const selected = o.id === (state.tag || "all") ? " selected" : "";
      return `<option value="${escapeHtml(o.id)}"${selected}>${escapeHtml(label)}</option>`;
    })
    .join("");

  select.innerHTML = html;
  // Re-apply value after paint — some Chromium builds drop selectedIndex on bulk replace.
  select.value = state.tag || "all";
  if (!select.value) {
    select.value = "all";
    state.tag = "all";
  }
}

function renderTimeline() {
  const list = filteredChanges();
  const el = document.getElementById("timeline");
  const count = document.getElementById("changes-count");
  if (count) {
    count.textContent =
      list.length === state.changes.length
        ? `${list.length} commits`
        : `${list.length} of ${state.changes.length} commits`;
  }
  if (!el) return;
  if (!list.length) {
    el.innerHTML = `<p class="empty-state">No matching changes. Clear search or pick another feature.</p>`;
    return;
  }

  const groups = [];
  for (const c of list) {
    const key = dayKey(c.date);
    if (!groups.length || groups[groups.length - 1].key !== key) {
      groups.push({ key, label: dayLabel(c.date), items: [c] });
    } else {
      groups[groups.length - 1].items.push(c);
    }
  }

  el.innerHTML = groups
    .map((g) => {
      const rows = g.items
        .map((c) => {
          const primary = (c.impact.features || [])[0];
          const extra = Math.max(0, (c.impact.features || []).length - 1);
          const impact = primary
            ? `<span class="impact-pill ${escapeHtml(primary.severity)}">${escapeHtml(
                primary.name
              )}${extra ? ` +${extra}` : ""}</span>`
            : `<span class="impact-pill muted">Unmapped</span>`;
          const files = (c.files || []).length;
          return `
            <button type="button" class="change-row" data-sha="${escapeHtml(c.sha)}">
              <span class="col-commit">
                <span class="sha">${escapeHtml(c.short_sha)}</span>
                <span class="subject">${escapeHtml(c.subject)}</span>
              </span>
              <span class="col-author" title="${escapeHtml(c.author)}">${escapeHtml(
                c.author
              )}</span>
              <span class="col-when">${escapeHtml(formatWhen(c.date))}</span>
              <span class="col-diff">
                <span class="add">+${c.additions}</span>
                <span class="del">-${c.deletions}</span>
                <span class="files">${files}f</span>
              </span>
              <span class="col-impact">${impact}<span class="row-chevron" aria-hidden="true">›</span></span>
            </button>`;
        })
        .join("");
      return `
        <section class="change-group">
          <h3 class="change-day">${escapeHtml(g.label)}</h3>
          <div class="change-rows">${rows}</div>
        </section>`;
    })
    .join("");
}

async function openDrawer(sha) {
  const detail = await fetchJson(`/api/changes/${encodeURIComponent(sha)}`);
  const body = document.getElementById("drawer-body");
  document.getElementById("drawer-title").textContent = detail.subject;

  const features = (detail.impact.features || [])
    .map(
      (f) => `
      <div class="layer" style="margin-bottom:8px">
        <h3>${escapeHtml(f.name)} <span class="tag ${escapeHtml(f.severity)}">${escapeHtml(
          f.severity
        )}</span></h3>
        <p>${escapeHtml(f.user_impact)}</p>
        <p class="muted" style="margin-top:4px">${escapeHtml(f.why)}</p>
      </div>`
    )
    .join("");

  const dimensions = (detail.impact.dimensions || [])
    .map(
      (d) =>
        `<div class="tag ${d.level === "高" ? "high" : d.level === "中" ? "medium" : "low"}" style="display:inline-block;margin:0 6px 6px 0">${escapeHtml(
          d.label
        )} · ${escapeHtml(d.level)}：${escapeHtml(d.note)}</div>`
    )
    .join("");

  const checklist = (detail.impact.checklist || [])
    .map((i) => `<li>${escapeHtml(i)}</li>`)
    .join("");
  const improvements = (detail.impact.improvements || [])
    .map((i) => `<li>${escapeHtml(i)}</li>`)
    .join("");
  const risks = (detail.impact.risks || [])
    .map((r) => `<li>${escapeHtml(r)}</li>`)
    .join("");
  const files = (detail.files || [])
    .map(
      (f) => `
      <div class="file-row">
        <span>${escapeHtml(f.path)}</span>
        <span><span class="add">+${f.additions}</span> <span class="del">-${f.deletions}</span></span>
      </div>`
    )
    .join("");

  body.innerHTML = `
    <p class="muted">${escapeHtml(detail.author)} · ${escapeHtml(detail.date)} · <span class="sha">${escapeHtml(
      detail.short_sha
    )}</span></p>
    ${detail.body ? `<pre style="white-space:pre-wrap;color:var(--text-2)">${escapeHtml(detail.body)}</pre>` : ""}

    <h3>Product features</h3>
    ${features || "<span class='muted'>No catalog match (add Impact: in the commit message)</span>"}

    <h3>Impact dimensions</h3>
    <div>${dimensions || "<span class='muted'>None</span>"}</div>

    <h3>Suggested checklist</h3>
    <ul>${checklist || "<li class='muted'>No required regressions</li>"}</ul>

    <h3>Improvements</h3>
    <ul>${improvements || "<li class='muted'>None</li>"}</ul>

    <h3>Risks</h3>
    <ul>${risks || "<li class='muted'>No major risks flagged</li>"}</ul>

    <h3>Files (${detail.files.length})</h3>
    ${files}
  `;

  document.getElementById("drawer").classList.remove("hidden");
  document.getElementById("backdrop").classList.remove("hidden");
}

function closeDrawer() {
  document.getElementById("drawer").classList.add("hidden");
  document.getElementById("backdrop").classList.add("hidden");
}

async function refreshAll({ silent } = { silent: false }) {
  const [overview, arch, changes, features, workflow] = await Promise.all([
    fetchJson("/api/overview"),
    fetchJson("/api/architecture"),
    fetchJson("/api/changes?limit=80"),
    fetchJson("/api/features"),
    fetchJson("/api/workflow"),
  ]);

  state.changes = changes;
  state.features = features;
  state.workflow = workflow;
  renderStats(overview);
  renderFeatureMatrix(features);
  renderArchitecture(arch);
  renderWorkflow(workflow);
  renderFilters();
  if (state.page === "changes") {
    renderTimeline();
  }

  const el = document.getElementById("refresh-meta");
  if (el) {
    const now = new Date();
    const hh = String(now.getHours()).padStart(2, "0");
    const mm = String(now.getMinutes()).padStart(2, "0");
    const ss = String(now.getSeconds()).padStart(2, "0");
    const mods = overview.desktop_modules ?? overview.discovered_modules ?? "-";
    el.textContent = silent
      ? `Auto-refresh ${hh}:${mm}:${ss} · modules ${mods}`
      : `Loaded ${hh}:${mm}:${ss} · modules ${mods}`;
  }
}

function bindUiOnce() {
  document.getElementById("drawer-close")?.addEventListener("click", closeDrawer);
  document.getElementById("backdrop")?.addEventListener("click", closeDrawer);
  document.getElementById("lightbox-close")?.addEventListener("click", closeLightbox);
  document.getElementById("lightbox")?.addEventListener("click", (e) => {
    if (e.target.id === "lightbox") closeLightbox();
  });
  document.getElementById("btn-refresh")?.addEventListener("click", () => {
    refreshAll({ silent: false }).catch((err) => {
      const el = document.getElementById("refresh-meta");
      if (el) el.textContent = `Refresh failed: ${err.message}`;
    });
  });

  document.querySelectorAll(".nav-item").forEach((btn) => {
    btn.addEventListener("click", () => setPage(btn.dataset.page));
  });

  document.getElementById("filter-select")?.addEventListener("change", (e) => {
    state.tag = e.target.value || "all";
    state.filterMode = "feature";
    renderTimeline();
  });
  document.getElementById("change-search")?.addEventListener("input", (e) => {
    state.search = e.target.value || "";
    renderTimeline();
  });
  document.getElementById("code-map-layer")?.addEventListener("change", (e) => {
    state.codeMapLayer = e.target.value || "all";
    renderCodeMap();
  });
  document.getElementById("code-map-search")?.addEventListener("input", (e) => {
    state.codeMapSearch = e.target.value || "";
    renderCodeMap();
  });

  // Event delegation — survives auto-refresh re-renders.
  document.getElementById("feature-grid")?.addEventListener("click", (e) => {
    const card = e.target.closest(".feat");
    if (card?.dataset.feature) goFeatureFilter(card.dataset.feature);
  });
  document.getElementById("feature-grid")?.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const card = e.target.closest(".feat");
    if (!card?.dataset.feature) return;
    e.preventDefault();
    goFeatureFilter(card.dataset.feature);
  });

  document.getElementById("timeline")?.addEventListener("click", (e) => {
    const row = e.target.closest(".change-row");
    if (row?.dataset.sha) openDrawer(row.dataset.sha);
  });

  document.getElementById("diagrams")?.addEventListener("click", (e) => {
    const img = e.target.closest("img");
    if (!img) return;
    openLightbox(
      img.currentSrc || img.src,
      img.closest("figure")?.querySelector(".cap")?.textContent || img.alt
    );
  });

  const openWorkflowImage = (img, card) => {
    if (!img) return;
    const caption =
      img.closest("figure")?.querySelector("figcaption")?.textContent ||
      card?.querySelector("h3")?.textContent ||
      img.alt ||
      "";
    openLightbox(img.currentSrc || img.src, caption.trim());
  };

  document.getElementById("page-workflow")?.addEventListener("click", (e) => {
    const pipe = e.target.closest(".pipe-node");
    if (pipe?.dataset.scene) {
      highlightStep(pipe.dataset.scene, Number(pipe.dataset.step));
      return;
    }
    const toggle = e.target.closest(".scene-toggle");
    if (toggle) {
      const scene = toggle.closest(".scene");
      const id = scene?.dataset.scene;
      if (!id) return;
      if (scene.classList.contains("open")) {
        scene.classList.remove("open");
        toggle.setAttribute("aria-expanded", "false");
      } else {
        openScene(id, { scroll: false });
      }
      return;
    }
    const card = e.target.closest(".gallery-card");
    const img =
      e.target.closest(".hero-shot img, .scene-visual img") ||
      (card ? card.querySelector("img") : null);
    if (img) openWorkflowImage(img, card);
  });

  document.getElementById("page-workflow")?.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const card = e.target.closest(".gallery-card");
    if (!card) return;
    e.preventDefault();
    openWorkflowImage(card.querySelector("img"), card);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeLightbox();
      closeDrawer();
      return;
    }
    const t = e.target;
    const typing =
      t instanceof HTMLElement &&
      (t.tagName === "INPUT" ||
        t.tagName === "TEXTAREA" ||
        t.tagName === "SELECT" ||
        t.isContentEditable);
    if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "1") setPage("overview");
    if (e.key === "2") setPage("workflow");
    if (e.key === "3") setPage("changes");
  });

  window.addEventListener("hashchange", () => {
    const h = (location.hash || "").replace("#", "");
    if (h === "workflow" || h === "changes" || h === "overview") {
      if (state.page !== h) setPage(h);
    }
  });
}

async function boot() {
  bindUiOnce();

  // Load data first so filters/timeline have content, then apply route.
  await refreshAll({ silent: false });
  const hash = (location.hash || "").replace("#", "");
  if (hash === "workflow" || hash === "changes" || hash === "overview") {
    setPage(hash);
  } else {
    setPage("overview");
  }

  setInterval(() => {
    refreshAll({ silent: true }).catch((err) => {
      const el = document.getElementById("refresh-meta");
      if (el) el.textContent = `Refresh failed: ${err.message}`;
    });
  }, 12000);
}

boot().catch((err) => {
  document.getElementById("top-meta").textContent = `Failed: ${err.message}`;
  console.error(err);
});
