const $ = (id) => document.getElementById(id);

function setTheme(theme) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("theme", theme);
  $("theme").textContent = theme === "dark" ? "Light" : "Dark";
}

function initTheme() {
  const saved = localStorage.getItem("theme");
  const prefersLight = window.matchMedia("(prefers-color-scheme: light)").matches;
  setTheme(saved || (prefersLight ? "light" : "dark"));
}

function timeAgo(iso) {
  if (!iso) return "-";
  const seconds = (Date.now() - new Date(iso).getTime()) / 1000;
  if (seconds < 60) return `${Math.max(0, Math.floor(seconds))}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

function escapeHtml(text) {
  return String(text).replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
}

function render(data) {
  $("status").textContent = data.status;
  $("status").className = "value " + (data.status === "ok" ? "ok" : "bad");
  $("last-run").textContent = data.last_run
    ? `${timeAgo(data.last_run)} (${new Date(data.last_run).toLocaleString()})`
    : "-";
  $("updates-sent").textContent = data.updates_sent;
  $("user-count").textContent = data.users.length;
  $("started").textContent = new Date(data.started_at).toLocaleString();

  $("users-body").innerHTML =
    data.users
      .map(
        (user) => `
      <tr>
        <td>${escapeHtml(user.name)} ${user.admin ? '<span class="tag admin">admin</span>' : ""}</td>
        <td>${user.username ? "@" + escapeHtml(user.username) : "-"}</td>
        <td class="mono">${user.chat_id}</td>
        <td>${user.active ? '<span class="tag ok">active</span>' : '<span class="tag bad">inactive</span>'}</td>
      </tr>`,
      )
      .join("") ||
    '<tr><td colspan="4" class="muted">No users yet. Send /start to the bot.</td></tr>';

  $("updates-list").innerHTML =
    data.recent
      .map(
        (update) => `
      <article class="card update">
        <div class="row">
          <span class="tag ${update.kind === "Flash" ? "flash" : "latest"}">${update.kind}</span>
          <span class="muted">${timeAgo(update.at)}</span>
        </div>
        <p class="title">${escapeHtml(update.title)}</p>
        ${update.link ? `<a href="${escapeHtml(update.link)}" target="_blank" rel="noopener">${escapeHtml(update.link)}</a>` : ""}
        ${update.attachments ? `<p class="muted">${update.attachments} attachment(s)</p>` : ""}
      </article>`,
      )
      .join("") || '<p class="muted empty">No updates yet.</p>';
}

async function refresh() {
  try {
    const response = await fetch("/api/analytics");
    if (!response.ok) throw new Error(response.status);
    render(await response.json());
  } catch {
    $("status").textContent = "unreachable";
    $("status").className = "value bad";
  }
}

document.querySelectorAll("#tabs button").forEach((button) => {
  button.addEventListener("click", () => {
    document
      .querySelectorAll("#tabs button")
      .forEach((other) => other.classList.toggle("active", other === button));
    document
      .querySelectorAll("main > section")
      .forEach((section) => (section.hidden = section.id !== button.dataset.tab));
  });
});

$("theme").addEventListener("click", () => {
  setTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
});

initTheme();
refresh();
setInterval(refresh, 5000);
