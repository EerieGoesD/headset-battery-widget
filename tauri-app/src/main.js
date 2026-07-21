const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const { listen } = window.__TAURI__.event;

const appWindow = getCurrentWindow();
const isMac = navigator.userAgent.toLowerCase().includes("mac");

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
let batteryStarted = false;

// Licensing / trial state.
const TRIAL_DAYS = 7;
let owned = false;
let trialStartMs = 0;
let trialActive = false;
let hasAccess = true;
let needsTrialStart = false;
let unlockPrice = ""; // localized App Store price (e.g. "4,99 €"); empty until fetched

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

// Whichever of the battery / trial / paywall cards is currently visible.
function visibleCard() {
  for (const id of ["card", "trialCard", "lockCard"]) {
    const el = $(id);
    if (el && el.style.display !== "none") return el;
  }
  return $("card");
}

// Resize the OS window to hug the visible card (handles compact / screen changes).
async function syncSize() {
  await new Promise((r) => requestAnimationFrame(() => r()));
  const rect = visibleCard().getBoundingClientRect();
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

// ----- Licensing: 7-day trial + one-time lifetime unlock -----
//
// For previewing the screens on non-store builds (e.g. Windows), set localStorage
// "hbw_dev_license" to "start", "trial", or "expired".
function devLicenseMode() {
  return localStorage.getItem("hbw_dev_license");
}

async function initLicensing() {
  const dev = devLicenseMode();
  const storeBuild = isMac || !!dev;

  // Non-store builds (Windows/Linux/dev) stay fully open; Windows monetization is
  // handled separately by the Microsoft Store.
  if (!storeBuild) {
    owned = true;
    hasAccess = true;
    needsTrialStart = false;
    computeAccessAndUI();
    return;
  }

  // Hide everything until we know the license, so the battery does not flash first.
  for (const id of ["card", "trialCard", "lockCard"]) {
    const el = $(id);
    if (el) el.style.display = "none";
  }
  await syncSize();

  if (dev) {
    owned = false;
    unlockPrice = "4,99 €"; // sample for previewing the UI only (real price is from the store)
    if (dev === "start") trialStartMs = 0;
    else if (dev === "trial") trialStartMs = Date.now() - 2 * 86400000;
    else trialStartMs = Date.now() - 8 * 86400000; // expired
    computeAccessAndUI();
    return;
  }

  try {
    const status = JSON.parse(await invoke("iap_status"));
    owned = !!status.owned;
    trialStartMs = Number(status.trialStartMs) || 0;
  } catch {
    owned = false;
    trialStartMs = 0;
  }
  try {
    unlockPrice = await invoke("iap_price"); // localized price from the App Store
  } catch {
    unlockPrice = "";
  }
  computeAccessAndUI();
}

function computeAccessAndUI() {
  if (owned) {
    hasAccess = true;
    trialActive = false;
    needsTrialStart = false;
    applyLicenseUI(0);
    return;
  }
  if (trialStartMs > 0) {
    const msLeft = trialStartMs + TRIAL_DAYS * 86400000 - Date.now();
    trialActive = msLeft > 0;
    needsTrialStart = false;
    hasAccess = trialActive;
    applyLicenseUI(Math.max(0, Math.ceil(msLeft / 86400000)));
    return;
  }
  // No trial started yet, not owned.
  trialActive = false;
  needsTrialStart = true;
  hasAccess = false;
  applyLicenseUI(0);
}

function applyLicenseUI(daysLeft) {
  hideLicenseErrors();

  const tp = $("trialPrice");
  if (tp) tp.textContent = unlockPrice ? "of " + unlockPrice + " " : "";
  const buyBtn = $("btnUnlock");
  if (buyBtn) buyBtn.textContent = unlockPrice ? "Unlock - " + unlockPrice : "Unlock";

  if (needsTrialStart) {
    showScreen("trialCard");
    return;
  }
  if (!hasAccess) {
    showScreen("lockCard");
    return;
  }

  // Owned or trialing: show the battery widget.
  const bar = $("trialBar");
  if (!owned && trialActive) {
    $("trialText").textContent =
      daysLeft === 1 ? "1 day left in your free trial" : daysLeft + " days left in your free trial";
    bar.style.display = "";
  } else {
    bar.style.display = "none";
  }
  showScreen("card");
  startBattery();
}

function showScreen(id) {
  for (const s of ["card", "trialCard", "lockCard"]) {
    const el = $(s);
    if (el) el.style.display = s === id ? "" : "none";
  }
  syncSize();
}

function startBattery() {
  if (batteryStarted) return;
  batteryStarted = true;
  refresh();
}

async function startTrial() {
  hideLicenseErrors();
  if (devLicenseMode()) {
    trialStartMs = Date.now();
    computeAccessAndUI();
    return;
  }
  try {
    const ms = await invoke("iap_start_trial");
    trialStartMs = Number(ms) || Date.now();
    computeAccessAndUI();
  } catch (e) {
    showLicenseError(friendlyStoreError(e)); syncSize();
  }
}

async function buyUnlock() {
  hideLicenseErrors();
  if (devLicenseMode()) {
    owned = true;
    computeAccessAndUI();
    return;
  }
  try {
    const ok = await invoke("iap_buy");
    if (ok) {
      owned = true;
      computeAccessAndUI();
    }
  } catch (e) {
    showLicenseError(friendlyStoreError(e)); syncSize();
  }
}

async function restoreUnlock() {
  hideLicenseErrors();
  try {
    await invoke("iap_restore");
    const status = JSON.parse(await invoke("iap_status"));
    owned = !!status.owned;
    trialStartMs = Number(status.trialStartMs) || 0;
    if (!owned && trialStartMs === 0) {
      showLicenseError("No previous purchase was found for this Apple ID.");
    }
    computeAccessAndUI();
  } catch (e) {
    showLicenseError(friendlyStoreError(e)); syncSize();
  }
}

function friendlyStoreError(e) {
  const msg = String(e || "");
  if (msg.includes("store_unavailable")) return msg;
  if (msg.toLowerCase().includes("mac app store build")) return msg;
  return msg;
}

function showLicenseError(msg) {
  for (const id of ["trialError", "lockError"]) {
    const el = $(id);
    if (el) {
      el.textContent = msg;
      el.style.display = "";
    }
  }
}

function hideLicenseErrors() {
  for (const id of ["trialError", "lockError"]) {
    const el = $(id);
    if (el) el.style.display = "none";
  }
}

function wireLicense() {
  $("btnStartTrial").addEventListener("click", () => startTrial());
  $("btnRestoreTrial").addEventListener("click", () => restoreUnlock());
  $("btnUnlock").addEventListener("click", () => buyUnlock());
  $("btnRestoreLock").addEventListener("click", () => restoreUnlock());
  $("trialUnlock").addEventListener("click", () => buyUnlock());
  $("btnQuitTrial").addEventListener("click", () => invoke("quit"));
  $("btnQuitLock").addEventListener("click", () => invoke("quit"));
}

window.addEventListener("DOMContentLoaded", async () => {
  wireLinks();
  wireLicense();
  await initControls();
  await applyInitialState();
  listen("refresh-now", () => {
    if (hasAccess) refresh();
  });
  listen("open-diagnostics", () => {
    if (hasAccess) openDiagnostics();
  });
  await initLicensing();
});
