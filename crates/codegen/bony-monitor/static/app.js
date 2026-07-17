const state = {
  changes: [],
  features: null,
  tag: "all",
  filterMode: "feature", // feature | area
};

const AREA_LABELS = {
  all: "全部",
  desktop: "桌面模块",
  monitor: "监控模块",
  "agent-runtime": "运行时模块",
  tui: "TUI 模块",
  docs: "文档模块",
  workspace: "构建模块",
  config: "配置模块",
  other: "其他",
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

function renderStats(overview) {
  const el = document.getElementById("stats");
  el.innerHTML = [
    ["提交数", overview.commit_count],
    ["新增行", overview.additions],
    ["删除行", overview.deletions],
    ["触及功能", overview.features_touched],
    ["功能目录", overview.features_total],
  ]
    .map(
      ([label, value]) => `
      <div class="stat">
        <div class="label">${escapeHtml(label)}</div>
        <div class="value">${fmtNum(value)}</div>
      </div>`
    )
    .join("");

  document.getElementById("top-meta").innerHTML = `
    <div>${escapeHtml(overview.repo)}</div>
    <div>最新：${escapeHtml(overview.latest?.short_sha ?? "-")} · ${escapeHtml(
      overview.latest?.subject ?? ""
    )}</div>
  `;
}

function renderFeatureMatrix(features) {
  const el = document.getElementById("feature-grid");
  el.innerHTML = (features.activity || [])
    .map((a) => {
      const f = a.feature;
      const last = a.commit_count
        ? `${escapeHtml(a.last_sha)} · ${escapeHtml(a.last_subject)}`
        : "近期无改动";
      return `
        <article class="feat ${escapeHtml(a.heat)}" data-feature="${escapeHtml(f.id)}">
          <div class="feat-top">
            <h3>${escapeHtml(f.name)}</h3>
            <span class="heat">${escapeHtml(a.heat)} · ${a.commit_count} commits</span>
          </div>
          <div class="cat">${escapeHtml(f.category)}${f.user_facing ? " · 用户可见" : " · 内部"}</div>
          <p>${escapeHtml(f.description)}</p>
          <div class="feat-meta">
            <span class="add">+${a.additions}</span>
            <span class="del">-${a.deletions}</span>
            · ${last}
          </div>
        </article>`;
    })
    .join("");

  el.querySelectorAll(".feat").forEach((card) => {
    card.style.cursor = "pointer";
    card.addEventListener("click", () => {
      state.tag = card.dataset.feature;
      state.filterMode = "feature";
      renderFilters();
      renderTimeline();
      document.getElementById("timeline").scrollIntoView({ behavior: "smooth", block: "start" });
    });
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

function renderFilters() {
  const featureIds = (state.features?.activity || [])
    .filter((a) => a.commit_count > 0)
    .map((a) => a.feature.id);
  const labels = Object.fromEntries(
    (state.features?.activity || []).map((a) => [a.feature.id, a.feature.name])
  );

  const tags = ["all", ...featureIds];
  const el = document.getElementById("filters");
  el.innerHTML = tags
    .map((tag) => {
      const label = tag === "all" ? "全部功能" : labels[tag] || AREA_LABELS[tag] || tag;
      const active = state.tag === tag ? "active" : "";
      return `<button class="filter-btn ${active}" data-tag="${escapeHtml(tag)}">${escapeHtml(
        label
      )}</button>`;
    })
    .join("");

  el.querySelectorAll("button").forEach((btn) => {
    btn.addEventListener("click", () => {
      state.tag = btn.dataset.tag;
      state.filterMode = "feature";
      renderFilters();
      renderTimeline();
    });
  });
}

function renderTimeline() {
  const list =
    state.tag === "all"
      ? state.changes
      : state.changes.filter((c) =>
          (c.impact.features || []).some((f) => f.id === state.tag) ||
          (c.impact.tags || []).includes(state.tag)
        );

  const el = document.getElementById("timeline");
  if (!list.length) {
    el.innerHTML = `<p class="muted">没有匹配的改动。</p>`;
    return;
  }

  el.innerHTML = list
    .map((c) => {
      const featureTags = (c.impact.features || [])
        .slice(0, 5)
        .map(
          (a) =>
            `<span class="tag ${escapeHtml(a.severity)}">${escapeHtml(a.name)}</span>`
        )
        .join("");
      const dims = (c.impact.dimensions || [])
        .filter((d) => d.level === "高")
        .slice(0, 3)
        .map((d) => `<span class="tag high">${escapeHtml(d.label)}·高</span>`)
        .join("");
      const improvements = (c.impact.improvements || [])
        .filter((i) => i.includes("【功能"))
        .slice(0, 3)
        .map((i) => `<li>${escapeHtml(i)}</li>`)
        .join("");
      return `
        <article class="card" data-sha="${escapeHtml(c.sha)}">
          <div class="card-top">
            <h3>${escapeHtml(c.subject)}</h3>
            <span class="sha">${escapeHtml(c.short_sha)}</span>
          </div>
          <div class="card-meta">
            ${escapeHtml(c.author)} · ${escapeHtml(c.date)} ·
            <span class="add">+${c.additions}</span>
            <span class="del">-${c.deletions}</span>
          </div>
          <div class="tags">${featureTags}${dims}</div>
          ${improvements ? `<ul class="improvements">${improvements}</ul>` : ""}
        </article>`;
    })
    .join("");

  el.querySelectorAll(".card").forEach((card) => {
    card.addEventListener("click", () => openDrawer(card.dataset.sha));
  });
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
    ${detail.body ? `<pre style="white-space:pre-wrap;color:var(--muted)">${escapeHtml(detail.body)}</pre>` : ""}

    <h3>影响的产品功能</h3>
    ${features || "<span class='muted'>未匹配到功能目录（可在 commit 写 Impact:）</span>"}

    <h3>影响维度</h3>
    <div>${dimensions || "<span class='muted'>无</span>"}</div>

    <h3>建议验证清单</h3>
    <ul>${checklist || "<li class='muted'>无强制回归项</li>"}</ul>

    <h3>改进说明</h3>
    <ul>${improvements || "<li class='muted'>无</li>"}</ul>

    <h3>风险 / 注意</h3>
    <ul>${risks || "<li class='muted'>无明显风险标记</li>"}</ul>

    <h3>文件（${detail.files.length}）</h3>
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
  const [overview, arch, changes, features] = await Promise.all([
    fetchJson("/api/overview"),
    fetchJson("/api/architecture"),
    fetchJson("/api/changes?limit=80"),
    fetchJson("/api/features"),
  ]);

  state.changes = changes;
  state.features = features;
  renderStats(overview);
  renderFeatureMatrix(features);
  renderArchitecture(arch);
  renderFilters();
  renderTimeline();

  const el = document.getElementById("refresh-meta");
  if (el) {
    const now = new Date();
    const hh = String(now.getHours()).padStart(2, "0");
    const mm = String(now.getMinutes()).padStart(2, "0");
    const ss = String(now.getSeconds()).padStart(2, "0");
    const mods = overview.desktop_modules ?? overview.discovered_modules ?? "-";
    el.textContent = silent
      ? `已自动刷新 · ${hh}:${mm}:${ss} · 桌面模块 ${mods}`
      : `已加载 · ${hh}:${mm}:${ss} · 桌面模块 ${mods}`;
  }
}

async function boot() {
  document.getElementById("drawer-close").addEventListener("click", closeDrawer);
  document.getElementById("backdrop").addEventListener("click", closeDrawer);

  await refreshAll({ silent: false });
  setInterval(() => {
    refreshAll({ silent: true }).catch((err) => {
      const el = document.getElementById("refresh-meta");
      if (el) el.textContent = `刷新失败：${err.message}`;
    });
  }, 12000);
}

boot().catch((err) => {
  document.getElementById("top-meta").textContent = `加载失败：${err.message}`;
  console.error(err);
});
