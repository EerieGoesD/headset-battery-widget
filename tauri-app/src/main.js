const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const { listen } = window.__TAURI__.event;

const appWindow = getCurrentWindow();

// Replace the WebView2 browser menu (Inspect / Save as / Print...) with our own
// native right-click menu: Refresh now, Diagnostics, Exit.
document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  invoke("show_context_menu").catch(() => {});
});

const FAST_POLL = 1000;
const NORMAL_POLL = 15000;

let pollTimer = null;
let refreshing = false;
let lowNotified = false;

const store = {
  get(key, fallback) {
    const raw = localStorage.getItem(key);
    if (raw === null) return fallback;
    try {
      return JSON.parse(raw);
    } catch {
      return fallback;
    }
  },
  set(key, value) {
    localStorage.setItem(key, JSON.stringify(value));
  },
};

const $ = (id) => document.getElementById(id);

// Resize the OS window to hug the card content (handles compact / settings changes).
async function syncSize() {
  await new Promise((r) => requestAnimationFrame(() => r()));
  const rect = $("card").getBoundingClientRect();
  const width = Math.max(1, Math.ceil(rect.width));
  const height = Math.max(1, Math.ceil(rect.height));
  try {
    await invoke("set_window_size", { width, height });
  } catch {}
}

function setFill(percent, unknown) {
  const fill = $("batteryFill");
  if (unknown) {
    fill.style.width = "100%";
    fill.style.opacity = "0.15";
    fill.style.background = "#8b8b96";
    return;
  }
  const pct = Math.max(0, Math.min(100, percent));
  fill.style.opacity = "1";
  fill.style.width = pct + "%";
  if (pct >= 50) fill.style.background = "#32cd32";
  else if (pct >= 20) fill.style.background = "#ffd700";
  else fill.style.background = "#ff4500";
}

async function maybeNotify(percent) {
  if (!store.get("notify", false)) return;
  const threshold = store.get("threshold", 20);
  if (percent <= threshold) {
    if (!lowNotified) {
      lowNotified = true;
      try {
        await invoke("notify", {
          title: "Headset battery low",
          body: `Battery at ${percent}%. Time to charge.`,
        });
      } catch {}
    }
  } else if (percent >= threshold + 5) {
    // Reset once it recovers, so it can alert again next drain.
    lowNotified = false;
  }
}

function schedule(ms) {
  if (pollTimer) clearTimeout(pollTimer);
  pollTimer = setTimeout(refresh, ms);
}

async function refresh() {
  if (refreshing) return;
  refreshing = true;
  try {
    const result = await invoke("read_battery");
    if (!result.success) {
      $("percentText").textContent = "--%";
      $("statusText").textContent = result.message;
      setFill(0, true);
      schedule(FAST_POLL);
      return;
    }
    const pct = Math.max(0, Math.min(100, result.percent));
    $("percentText").textContent = pct + "%";
    $("statusText").textContent = result.label || "Connected";
    setFill(pct, false);
    await maybeNotify(pct);
    schedule(NORMAL_POLL);
  } catch {
    $("statusText").textContent = "Error reading battery";
    setFill(0, true);
    schedule(FAST_POLL);
  } finally {
    refreshing = false;
  }
}

function setCompact(compact) {
  store.set("compact", compact);
  $("card").classList.toggle("compact", compact);
  $("btnCompact").style.display = compact ? "none" : "";
  $("btnExpand").style.display = compact ? "" : "none";
  if (compact) $("settingsPanel").style.display = "none";
  syncSize();
}

function toggleSettings() {
  const panel = $("settingsPanel");
  panel.style.display = panel.style.display === "none" ? "" : "none";
  syncSize();
}

async function populateDiagnostics() {
  const out = $("diagOut");
  try {
    const rows = await invoke("list_devices");
    out.textContent = rows.length
      ? rows
          .map(
            (d) =>
              `${d.manufacturer} ${d.product}\n  VID ${d.vid}  PID ${d.pid}  usage ${d.usage}/${d.usage_page}`
          )
          .join("\n")
      : "No supported devices found.";
  } catch (e) {
    out.textContent = "Error: " + e;
  }
}

// Opened from the right-click / tray "Diagnostics" item: reveal the panel and results.
async function openDiagnostics() {
  if ($("card").classList.contains("compact")) setCompact(false);
  $("settingsPanel").style.display = "";
  await populateDiagnostics();
  $("diagOut").style.display = "";
  await syncSize();
}

function wireLinks() {
  document.querySelectorAll("[data-url]").forEach((el) => {
    el.addEventListener("click", (e) => {
      e.preventDefault();
      invoke("open_external", { url: el.getAttribute("data-url") }).catch(() => {});
    });
  });
}

async function initControls() {
  // Always on top
  const chkTop = $("chkTopmost");
  chkTop.checked = store.get("topmost", true);
  chkTop.addEventListener("change", async () => {
    store.set("topmost", chkTop.checked);
    try {
      await appWindow.setAlwaysOnTop(chkTop.checked);
    } catch {}
  });

  // Low-battery alert toggle
  const chkNotify = $("chkNotify");
  chkNotify.checked = store.get("notify", false);
  chkNotify.addEventListener("change", () => store.set("notify", chkNotify.checked));

  // Threshold
  const numThreshold = $("numThreshold");
  numThreshold.value = store.get("threshold", 20);
  numThreshold.addEventListener("change", () => {
    let v = parseInt(numThreshold.value, 10);
    if (Number.isNaN(v)) v = 20;
    v = Math.max(1, Math.min(99, v));
    numThreshold.value = v;
    store.set("threshold", v);
  });

  // Launch at startup (state lives in the OS; read it from Rust)
  const chkAuto = $("chkAutostart");
  try {
    chkAuto.checked = await invoke("get_autostart");
  } catch {}
  chkAuto.addEventListener("change", async () => {
    try {
      await invoke("set_autostart", { enabled: chkAuto.checked });
    } catch {
      chkAuto.checked = !chkAuto.checked;
    }
  });

  // Hide taskbar button (the native tray icon always stays regardless)
  const chkHide = $("chkHideTaskbar");
  chkHide.checked = store.get("hideTaskbar", false);
  chkHide.addEventListener("change", async () => {
    store.set("hideTaskbar", chkHide.checked);
    try {
      await appWindow.setSkipTaskbar(chkHide.checked);
    } catch {}
  });

  // Buttons
  $("btnCompact").addEventListener("click", () => setCompact(true));
  $("btnExpand").addEventListener("click", () => setCompact(false));
  $("btnSettings").addEventListener("click", toggleSettings);
  $("btnQuit").addEventListener("click", () => invoke("quit"));

  // Diagnostics
  $("btnDiag").addEventListener("click", async () => {
    const out = $("diagOut");
    if (out.style.display === "none") {
      await populateDiagnostics();
      out.style.display = "";
    } else {
      out.style.display = "none";
    }
    await syncSize();
  });
}

async function applyInitialState() {
  try {
    await appWindow.setAlwaysOnTop(store.get("topmost", true));
  } catch {}
  if (store.get("hideTaskbar", false)) {
    try {
      await appWindow.setSkipTaskbar(true);
    } catch {}
  }
  if (store.get("compact", false)) setCompact(true);
}

window.addEventListener("DOMContentLoaded", async () => {
  wireLinks();
  await initControls();
  await applyInitialState();
  await syncSize();
  listen("refresh-now", () => refresh());
  listen("open-diagnostics", () => openDiagnostics());
  refresh();
});
