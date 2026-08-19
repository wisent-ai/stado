//! Escaped HTML rendering and cleanup API presentation for the dashboard.
//! Port of `stado/dashboard_summary/web_view.py` — operator-facing, so the
//! markup, scripts and text are reproduced verbatim.
//!
//! Values arrive as [`serde_json::Value`], so the Python
//! `dict.get(key, default)` / `x or default` idioms are re-expressed over
//! `Option<&Value>` with Python truthiness. One tolerance: Python's
//! `int(cleaner.get(...) or 0)` raises on garbage strings in a corrupt
//! state file (500 on the route); the Rust port clamps to 0 instead.

use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Python str()/truthiness over JSON values
// ---------------------------------------------------------------------------

/// Python `str(value)`: strings raw, null -> "None", bools True/False,
/// numbers in their JSON spelling.
fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python truthiness for the `x or default` fallback.
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `int(x or 0)` over JSON: numbers truncate toward zero, strings
/// parse, everything else (and garbage strings, see module doc) -> 0.
fn int_or_zero(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) as i64,
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Python `html.escape(s, quote=True)`.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Python `_e(value)`.
fn e(value: &Value) -> String {
    escape(&py_str(value))
}

/// Python `_e(x or default)` — falsy values fall back.
fn e_or(value: Option<&Value>, default: &str) -> String {
    match value {
        Some(v) if py_truthy(v) => e(v),
        _ => escape(default),
    }
}

/// Python `_e(map.get(key, default))` — the default applies only when the
/// key is absent, not when the value is falsy.
fn e_get(map: Option<&Map<String, Value>>, key: &str, default: &str) -> String {
    match map.and_then(|m| m.get(key)) {
        Some(v) => e(v),
        None => escape(default),
    }
}

/// Python `str(map.get(key, default))` without escaping (used inside
/// already-escaped compositions).
fn str_get(map: Option<&Map<String, Value>>, key: &str, default: &str) -> String {
    map.and_then(|m| m.get(key))
        .map_or_else(|| default.to_string(), py_str)
}

fn as_obj(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Python `_format_age`.
fn format_age(value: Option<&Value>) -> String {
    let seconds = match value {
        // bool is not a number in JSON terms either — excluded like Python.
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        _ => return "?".to_string(),
    };
    if seconds.is_nan() {
        return "?".to_string();
    }
    if seconds < 60.0 {
        return format!("{}s", (seconds as i64).max(0));
    }
    if seconds < 3600.0 {
        return format!("{}m{}s", (seconds / 60.0) as i64, (seconds % 60.0) as i64);
    }
    format!(
        "{}h{}m",
        (seconds / 3600.0) as i64,
        ((seconds % 3600.0) / 60.0) as i64
    )
}

/// Python `_bytes`: ints only -> "X.Y GiB", anything else "unknown".
fn bytes_gib(value: Option<&Value>) -> String {
    let Some(bytes) = value.and_then(Value::as_i64) else {
        return "unknown".to_string();
    };
    format!("{:.1} GiB", bytes as f64 / 1024_f64.powi(3))
}

// ---------------------------------------------------------------------------
// cleanup envelope + card
// ---------------------------------------------------------------------------

/// Python `cleanup_envelope`.
pub fn cleanup_envelope(report: &Value) -> Value {
    let busy = report.get("lock_busy") == Some(&Value::Bool(true))
        || report.get("outcome").and_then(Value::as_str) == Some("lock_busy");
    serde_json::json!({
        "ok": !busy,
        "service": if busy { "busy" } else { "ready" },
        "report": report,
    })
}

#[derive(Default)]
struct CleanupTotals {
    eligible: i64,
    deleted: i64,
    reclaimed: i64,
    skips: i64,
}

/// Python `_cleanup_totals`.
fn cleanup_totals(report: &Map<String, Value>) -> CleanupTotals {
    let mut totals = CleanupTotals::default();
    let Some(cleaners) = report.get("cleaners").and_then(Value::as_object) else {
        return totals;
    };
    for cleaner in cleaners.values() {
        let Some(cleaner) = cleaner.as_object() else {
            continue;
        };
        totals.eligible += int_or_zero(cleaner.get("eligible_items"));
        totals.deleted += int_or_zero(cleaner.get("deleted_items"));
        totals.reclaimed += int_or_zero(cleaner.get("actual_free_delta_bytes"));
        if let Some(skipped) = cleaner.get("skipped").and_then(Value::as_object) {
            totals.skips += skipped.values().map(|v| int_or_zero(Some(v))).sum::<i64>();
        }
    }
    totals
}

/// Python `_cleanup_card`. `cleanup` is the envelope from
/// [`cleanup_envelope`].
fn cleanup_card(cleanup: &Value) -> String {
    let empty = Map::new();
    let report = as_obj(cleanup.get("report")).unwrap_or(&empty);
    let totals = cleanup_totals(report);
    let caps = as_obj(report.get("caps"));
    let active_caps: Vec<&str> = ["bytes", "items", "scan", "deadline"]
        .into_iter()
        .filter(|name| caps.and_then(|c| c.get(*name)) == Some(&Value::Bool(true)))
        .collect();
    let active_caps = if active_caps.is_empty() {
        "none".to_string()
    } else {
        active_caps.join(", ")
    };
    let pressure_text = match report.get("pressure_active") {
        Some(Value::Bool(true)) => "active",
        Some(Value::Bool(false)) => "clear",
        _ => "unknown",
    };
    let error_count = report
        .get("errors")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let report = Some(report);
    format!(
        r#"
<section class="cleanup-card" aria-labelledby="cleanup-title">
  <div class="cleanup-heading">
    <div><h2 id="cleanup-title">Stado Disk Cleanup</h2>
      <p class="muted">Registry-controlled local cleanup; no custom paths or parameters.</p></div>
    <div class="controls">
      <button type="button" id="cleanup-refresh">Refresh</button>
      <button type="button" id="cleanup-run">Run cleanup</button>
    </div>
  </div>
  <p id="cleanup-status" class="service-status" role="status" aria-live="polite">
    Service {service}; outcome {outcome}.
  </p>
  <dl class="cleanup-grid">
    <div><dt>Mode / outcome</dt><dd>{mode} / {outcome2}</dd></div>
    <div><dt>Free / low / target</dt><dd>{free} / {low} / {target}</dd></div>
    <div><dt>Pressure</dt><dd>{pressure}</dd></div>
    <div><dt>Last success</dt><dd>{last_success}</dd></div>
    <div><dt>Caps reached</dt><dd>{caps}</dd></div>
    <div><dt>Eligible / deleted</dt><dd>{eligible} / {deleted}</dd></div>
    <div><dt>Reclaimed</dt><dd>{reclaimed}</dd></div>
    <div><dt>Skips / errors</dt><dd>{skips} / {errors}</dd></div>
  </dl>
</section>"#,
        service = e_get(as_obj(Some(cleanup)), "service", "unknown"),
        outcome = e_get(report, "outcome", "never_run"),
        mode = e_or(report.and_then(|r| r.get("mode")), "not configured"),
        outcome2 = e_or(report.and_then(|r| r.get("outcome")), "never_run"),
        free = bytes_gib(report.and_then(|r| r.get("free_bytes_after"))),
        low = bytes_gib(report.and_then(|r| r.get("low_bytes"))),
        target = bytes_gib(report.and_then(|r| r.get("target_bytes"))),
        pressure = pressure_text,
        last_success = e_or(report.and_then(|r| r.get("last_success_at")), "none"),
        caps = escape(&active_caps),
        eligible = totals.eligible,
        deleted = totals.deleted,
        reclaimed = bytes_gib(Some(&Value::from(totals.reclaimed))),
        skips = totals.skips,
        errors = error_count,
    )
}

/// Python `_policy_card` (verbatim markup).
fn policy_card() -> &'static str {
    r#"
<section class="cleanup-card" aria-labelledby="policy-title">
  <div class="cleanup-heading">
    <div><h2 id="policy-title">Registry disk policy</h2>
      <p class="muted">Per-target disk cleanup and Weles recordings location. Writes go through the canonical registry (generation-checked, whitelisted fields only).</p></div>
    <div class="controls"><button type="button" id="policy-refresh">Refresh</button></div>
  </div>
  <p id="policy-status" class="service-status" role="status" aria-live="polite"></p>
  <table id="policy-table">
    <thead><tr>
      <th>target</th><th>mode</th><th>low GB</th><th>target GB</th>
      <th>items/pass</th><th>bytes/pass (GiB)</th><th>weles min age (d)</th>
      <th>proof bypass</th><th>recordings root</th><th>pinned only</th><th></th>
    </tr></thead>
    <tbody></tbody>
  </table>
</section>"#
}

/// Python `_policy_script` (verbatim; talks to the generation-checked
/// canonical-registry policy endpoints served by `dashboard::policy`).
fn policy_script() -> &'static str {
    r#"
<script>
(() => {
  const status = document.getElementById('policy-status');
  const tbody = document.querySelector('#policy-table tbody');
  const numberFields = ['low_free_gb','target_free_gb','max_items_per_pass'];
  async function load() {
    status.textContent = 'Loading registry policy.';
    try {
      const response = await fetch('/api/registry.json');
      const data = await response.json();
      tbody.textContent = '';
      for (const t of data.targets || []) {
        const dc = t.disk_cleanup || {};
        const cl = ((dc.cleaners || {}).weles_recordings) || {};
        const row = document.createElement('tr');
        row.dataset.target = t.name;
        row.innerHTML =
          `<td><code>${t.name}</code></td>` +
          `<td><select data-k="mode">${['off','report','enforce'].map(m => `<option${m === dc.mode ? ' selected' : ''}>${m}</option>`).join('')}</select></td>` +
          `<td><input size="4" data-k="low_free_gb" value="${dc.low_free_gb ?? ''}"></td>` +
          `<td><input size="4" data-k="target_free_gb" value="${dc.target_free_gb ?? ''}"></td>` +
          `<td><input size="4" data-k="max_items_per_pass" value="${dc.max_items_per_pass ?? ''}"></td>` +
          `<td><input size="4" data-k="max_bytes_per_pass" value="${dc.max_bytes_per_pass != null ? Math.round(dc.max_bytes_per_pass / 1073741824) : ''}"></td>` +
          `<td><input size="4" data-k="weles_min_age_days" value="${cl.min_age_seconds != null ? Math.round(cl.min_age_seconds / 86400) : ''}"></td>` +
          `<td><input type="checkbox" data-k="allow_missing_upload_proof"${cl.allow_missing_upload_proof ? ' checked' : ''}></td>` +
          `<td><input size="16" data-k="recordings_dir" value="${(t.weles && t.weles.recordings_dir) || cl.root || ''}" placeholder="/abs/path"></td>` +
          `<td><input type="checkbox" data-k="pinned_only"${t.pinned_only ? ' checked' : ''}></td>` +
          `<td><button type="button" data-save>Save</button></td>`;
        tbody.appendChild(row);
      }
      status.textContent = `Registry generation ${data.generation}.`;
    } catch (_) { status.textContent = 'Registry policy request failed safely.'; }
  }
  tbody.addEventListener('click', async (event) => {
    const button = event.target.closest('button[data-save]');
    if (!button) return;
    const row = button.closest('tr');
    const values = {};
    row.querySelectorAll('[data-k]').forEach((input) => { values[input.dataset.k] = input.type === 'checkbox' ? input.checked : input.value.trim(); });
    const GiB = 1073741824, DAY = 86400;
    const payload = {
      target: row.dataset.target,
      disk_cleanup: {
        mode: values.mode,
        low_free_gb: Number(values.low_free_gb),
        target_free_gb: Number(values.target_free_gb),
        max_items_per_pass: Number(values.max_items_per_pass),
        max_bytes_per_pass: Number(values.max_bytes_per_pass) * GiB,
        cleaners: { weles_recordings: {
          min_age_seconds: Number(values.weles_min_age_days) * DAY,
          allow_missing_upload_proof: values.allow_missing_upload_proof,
        } },
      },
      pinned_only: values.pinned_only,
    };
    if (values.recordings_dir) payload.weles = { recordings_dir: values.recordings_dir };
    button.disabled = true;
    status.textContent = `Saving policy for ${row.dataset.target}.`;
    try {
      const response = await fetch('/api/registry/policy', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Stado-Action': 'registry-policy' },
        body: JSON.stringify(payload),
      });
      const result = await response.json();
      if (!response.ok || !result.ok) {
        status.textContent = `Rejected: ${result.error || response.status}`;
      } else {
        status.textContent = `Saved ${row.dataset.target} (generation ${result.generation}).`;
        window.setTimeout(load, 800);
      }
    } catch (_) { status.textContent = 'Registry policy save failed safely.'; }
    finally { button.disabled = false; }
  });
  document.getElementById('policy-refresh').addEventListener('click', load);
  load();
})();
</script>"#
}

fn operator_workspace() -> &'static str {
    r##"
<style>
.operator-shell{margin:1.4em 0;border:1px solid #c8ced8;border-radius:14px;overflow:hidden;background:#f8fafc;box-shadow:0 10px 30px rgba(15,23,42,.08)}
.operator-head{padding:1.2em 1.3em;background:linear-gradient(120deg,#172554,#1e3a8a);color:#fff}
.operator-head h2{margin:0;border:0;padding:0;color:#fff}.operator-head p{margin:.45em 0 0;color:#dbeafe;max-width:78ch}
.operator-grid{display:grid;grid-template-columns:minmax(190px,240px) minmax(0,1fr);min-height:560px}
.operator-nav{padding:.8em;background:#eef2ff;border-right:1px solid #cbd5e1;display:flex;flex-direction:column;gap:.28em}
.operator-nav button{border:0;background:transparent;text-align:left;padding:.65em .75em;border-radius:8px;color:#334155;font-weight:600;cursor:pointer}
.operator-nav button:hover,.operator-nav button[aria-current=true]{background:#fff;color:#1d4ed8;box-shadow:0 1px 4px rgba(15,23,42,.12)}
.operator-main{padding:1.2em;min-width:0}.operator-main h3{margin:.1em 0 .3em;font-size:1.25em}.operator-description{color:#475569;margin:0 0 1em}
.operator-form{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:1em}.operator-field{display:flex;flex-direction:column;gap:.35em}
.operator-field-wide{grid-column:1/-1}.operator-field label{font-size:12px;font-weight:700;text-transform:uppercase;letter-spacing:.04em;color:#475569}
.operator-field select,.operator-field input,.operator-field textarea{box-sizing:border-box;width:100%;border:1px solid #94a3b8;border-radius:7px;padding:.65em;background:#fff;color:#0f172a;font:13px ui-monospace,SFMono-Regular,Menlo,monospace}
.operator-field select{font-family:-apple-system,system-ui,sans-serif}.operator-field textarea{resize:vertical;min-height:122px}
.operator-action-note{min-height:2.8em;color:#475569;font-size:13px;margin:.3em 0}
.operator-controls{display:flex;flex-wrap:wrap;align-items:center;gap:.8em;grid-column:1/-1}
.operator-controls label{display:flex;align-items:center;gap:.45em;color:#7f1d1d;font-weight:600}.operator-controls input[type=checkbox]{width:auto}
.operator-run{border:0;border-radius:8px;padding:.7em 1.15em;background:#2563eb;color:#fff;font-weight:700;cursor:pointer}.operator-run:disabled{opacity:.55;cursor:wait}
.operator-status{font-size:13px;color:#475569}.operator-output{grid-column:1/-1;margin-top:.2em}.operator-output-head{display:flex;justify-content:space-between;align-items:center}
.operator-output pre{margin:.45em 0 0;min-height:170px;max-height:480px;overflow:auto;padding:1em;border-radius:8px;background:#0f172a;color:#dbeafe;white-space:pre-wrap;overflow-wrap:anywhere}
.operator-badge{display:inline-block;border-radius:999px;padding:.2em .55em;font-size:11px;font-weight:700;background:#dbeafe;color:#1e40af}.operator-badge.mutating{background:#fee2e2;color:#991b1b}
.operator-history{grid-column:1/-1}.operator-history ol{margin:.4em 0;padding-left:1.4em;color:#475569;font-size:12px}.operator-history button{border:0;background:none;color:#1d4ed8;cursor:pointer;padding:0}
@media(max-width:760px){.operator-grid{grid-template-columns:1fr}.operator-nav{border-right:0;border-bottom:1px solid #cbd5e1;display:grid;grid-template-columns:repeat(2,minmax(0,1fr))}.operator-form{grid-template-columns:1fr}.operator-field-wide,.operator-controls,.operator-output,.operator-history{grid-column:1}}
@media(prefers-color-scheme:dark){.operator-shell{background:#111827;border-color:#475569}.operator-nav{background:#1e293b;border-color:#475569}.operator-nav button{color:#cbd5e1}.operator-nav button:hover,.operator-nav button[aria-current=true]{background:#334155;color:#bfdbfe}.operator-main h3{color:#f8fafc}.operator-description,.operator-action-note,.operator-status,.operator-history ol{color:#cbd5e1}.operator-field label{color:#cbd5e1}.operator-field select,.operator-field input,.operator-field textarea{background:#0f172a;color:#e2e8f0;border-color:#64748b}}
</style>
<section class="operator-shell" aria-labelledby="operator-title">
  <header class="operator-head">
    <h2 id="operator-title">Operator workspace</h2>
    <p>Every Stado operator workflow through structured argv, the installed release, and the same policy boundaries as the CLI. No shell is exposed.</p>
  </header>
  <div class="operator-grid">
    <nav class="operator-nav" id="operator-nav" aria-label="Operator workflow"></nav>
    <div class="operator-main">
      <h3 id="operator-section-title">Loading operator catalog</h3>
      <p class="operator-description" id="operator-section-description"></p>
      <div class="operator-form">
        <div class="operator-field">
          <label for="operator-action">Operation</label>
          <select id="operator-action"></select>
          <p class="operator-action-note" id="operator-action-note"></p>
        </div>
        <div class="operator-field">
          <label for="operator-timeout">Time limit in seconds</label>
          <input id="operator-timeout" type="number" min="1" max="300" value="120">
          <p class="operator-action-note">Finite commands only; output is bounded to 1 MiB per stream.</p>
        </div>
        <div class="operator-field operator-field-wide">
          <label for="operator-args">Argument vector (JSON array; editable)</label>
          <textarea id="operator-args" spellcheck="false" aria-describedby="operator-args-help"></textarea>
          <span id="operator-args-help" class="muted">Arguments are passed directly to Stado. They are never parsed by a shell.</span>
        </div>
        <div class="operator-field operator-field-wide" id="operator-input-wrap">
          <label for="operator-input">Input file content (used when an argument is <code>$INPUT</code>)</label>
          <textarea id="operator-input" spellcheck="false" placeholder="Paste the complete JSON document here when the selected operation uses $INPUT."></textarea>
        </div>
        <div class="operator-controls">
          <label><input id="operator-confirm" type="checkbox"> Confirm policy-changing operation</label>
          <button class="operator-run" type="button" id="operator-run">Run operation</button>
          <span class="operator-badge" id="operator-risk">read only</span>
          <span class="operator-status" id="operator-status" role="status" aria-live="polite"></span>
        </div>
        <div class="operator-output">
          <div class="operator-output-head"><strong>Recorded command result</strong><button type="button" id="operator-copy">Copy output</button></div>
          <pre id="operator-result" tabindex="0">Choose an operation to inspect its exact argument vector.</pre>
        </div>
        <div class="operator-history">
          <strong>Session history</strong>
          <ol id="operator-history"></ol>
        </div>
      </div>
    </div>
  </div>
</section>
<script>
(() => {
  const state = { catalog: null, section: null, action: null, history: [] };
  const nav = document.getElementById('operator-nav');
  const sectionTitle = document.getElementById('operator-section-title');
  const sectionDescription = document.getElementById('operator-section-description');
  const actionSelect = document.getElementById('operator-action');
  const actionNote = document.getElementById('operator-action-note');
  const argsEditor = document.getElementById('operator-args');
  const inputEditor = document.getElementById('operator-input');
  const inputWrap = document.getElementById('operator-input-wrap');
  const confirm = document.getElementById('operator-confirm');
  const timeout = document.getElementById('operator-timeout');
  const risk = document.getElementById('operator-risk');
  const run = document.getElementById('operator-run');
  const status = document.getElementById('operator-status');
  const result = document.getElementById('operator-result');
  const history = document.getElementById('operator-history');
  const renderHistory = () => {
    history.textContent = '';
    for (const entry of state.history) {
      const item = document.createElement('li');
      const button = document.createElement('button');
      button.type = 'button';
      button.textContent = `${entry.ok ? 'ok' : 'failed'} · ${entry.args.join(' ')}`;
      button.addEventListener('click', () => { result.textContent = entry.output; result.focus(); });
      item.appendChild(button);
      history.appendChild(item);
    }
  };
  const selectAction = (id) => {
    state.action = state.section.actions.find((candidate) => candidate.id === id) || state.section.actions[0];
    if (!state.action) return;
    actionSelect.value = state.action.id;
    actionNote.textContent = state.action.description;
    argsEditor.value = JSON.stringify(state.action.args, null, 2);
    inputWrap.hidden = !state.action.input;
    if (!state.action.input) inputEditor.value = '';
    confirm.checked = false;
    risk.textContent = state.action.read_only ? 'read only' : 'changes state';
    risk.classList.toggle('mutating', !state.action.read_only);
    status.textContent = '';
  };
  const selectSection = (id) => {
    state.section = state.catalog.sections.find((candidate) => candidate.id === id) || state.catalog.sections[0];
    nav.querySelectorAll('button').forEach((button) => button.setAttribute('aria-current', String(button.dataset.id === state.section.id)));
    sectionTitle.textContent = state.section.label;
    sectionDescription.textContent = state.section.description;
    actionSelect.textContent = '';
    for (const action of state.section.actions) {
      const option = document.createElement('option');
      option.value = action.id;
      option.textContent = action.label;
      actionSelect.appendChild(option);
    }
    selectAction(state.section.actions[0]?.id);
  };
  const load = async () => {
    status.textContent = 'Loading catalog.';
    try {
      const response = await fetch('/api/operator/catalog');
      const payload = await response.json();
      if (!response.ok) throw new Error(payload.error || `HTTP ${response.status}`);
      state.catalog = payload;
      nav.textContent = '';
      for (const section of payload.sections) {
        const button = document.createElement('button');
        button.type = 'button';
        button.dataset.id = section.id;
        button.textContent = section.label;
        button.addEventListener('click', () => selectSection(section.id));
        nav.appendChild(button);
      }
      selectSection(payload.sections[0]?.id);
      status.textContent = `${payload.sections.length} workflow groups available.`;
    } catch (error) {
      sectionTitle.textContent = 'Operator catalog unavailable';
      status.textContent = error instanceof Error ? error.message : 'Catalog request failed.';
    }
  };
  actionSelect.addEventListener('change', () => selectAction(actionSelect.value));
  argsEditor.addEventListener('input', () => {
    status.textContent = 'Argument vector edited; the server will classify its risk.';
    risk.textContent = 'server classified';
    risk.classList.add('mutating');
  });
  run.addEventListener('click', async () => {
    let args;
    try {
      args = JSON.parse(argsEditor.value);
      if (!Array.isArray(args) || !args.every((value) => typeof value === 'string')) throw new Error('Argument vector must be an array of strings.');
    } catch (error) {
      status.textContent = error instanceof Error ? error.message : 'Invalid argument vector.';
      return;
    }
    run.disabled = true;
    status.textContent = 'Operation running.';
    result.textContent = 'Waiting for the recorded Stado result…';
    try {
      const response = await fetch('/api/operator/run', {
        method: 'POST',
        headers: {'Content-Type': 'application/json', 'X-Stado-Action': 'operator-command'},
        body: JSON.stringify({
          args,
          input: inputEditor.value || null,
          confirmation: confirm.checked ? 'RUN_MUTATION' : '',
          timeout_seconds: Number(timeout.value),
        }),
      });
      const payload = await response.json();
      const rendered = payload.structured != null
        ? JSON.stringify(payload.structured, null, 2)
        : [payload.stdout, payload.stderr].filter(Boolean).join('\\n');
      const output = rendered || payload.error || `Exit code ${payload.exit_code ?? 'unknown'} with no output.`;
      result.textContent = output;
      const ok = response.ok && payload.ok;
      status.textContent = ok ? `Completed with exit code ${payload.exit_code}.` : `Rejected or failed: ${payload.error || payload.exit_code || response.status}.`;
      state.history.unshift({ok, args, output});
      state.history = state.history.slice(0, 8);
      renderHistory();
    } catch (error) {
      result.textContent = error instanceof Error ? error.message : 'Operator request failed.';
      status.textContent = 'Operator request failed safely.';
    } finally {
      run.disabled = false;
    }
  });
  document.getElementById('operator-copy').addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(result.textContent || ''); status.textContent = 'Output copied.'; }
    catch (_) { status.textContent = 'Clipboard access was unavailable.'; }
  });
  load();
})();
</script>"##
}

// ---------------------------------------------------------------------------
// render_html
// ---------------------------------------------------------------------------

/// Python `render_html(state, cleanup, refresh)`.
pub fn render_html(state: &Value, cleanup: &Value, refresh: i64) -> String {
    let counts = as_obj(state.get("counts"));
    let throughput = as_obj(state.get("throughput"));
    let autonomy = as_obj(state.get("autonomy"));
    let forecast = as_obj(autonomy.and_then(|value| value.get("forecast")));
    let savings = as_obj(autonomy.and_then(|value| value.get("savings")));
    let anomaly_count = autonomy
        .and_then(|value| value.get("anomalies"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();

    let mut model_rows = String::new();
    let empty_models = Map::new();
    let models = as_obj(state.get("by_model_state")).unwrap_or(&empty_models);
    let mut model_items: Vec<(&String, &Value)> = models.iter().collect();
    // Python: sorted by -sum of the four counters (stable for ties).
    model_items.sort_by_key(|(_, row)| {
        std::cmp::Reverse(
            ["queue", "running", "completed", "failed"]
                .iter()
                .map(|k| int_or_zero(row.get(*k)))
                .sum::<i64>(),
        )
    });
    for (model, row) in model_items {
        model_rows.push_str("<tr>");
        for value in [
            e(&Value::String((*model).clone())),
            row.get("queue").map_or_else(|| "0".to_string(), e),
            row.get("running").map_or_else(|| "0".to_string(), e),
            row.get("completed").map_or_else(|| "0".to_string(), e),
            row.get("failed").map_or_else(|| "0".to_string(), e),
        ] {
            model_rows.push_str(&format!("<td>{value}</td>"));
        }
        model_rows.push_str("</tr>");
    }

    let mut agent_rows = String::new();
    let agents = state.get("live_agents").and_then(Value::as_array);
    if let Some(agents) = agents {
        for agent in agents {
            let agent = as_obj(Some(agent));
            let slots = agent
                .and_then(|a| a.get("free_slots"))
                .and_then(Value::as_object)
                .map(|free| {
                    free.iter()
                        .map(|(k, v)| format!("{}:{}", k, py_str(v)))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let values = [
                e_get(agent, "consumer_id", "None"),
                e_get(agent, "kind", "None"),
                escape(&slots),
                escape(&format!(
                    "{}/{}",
                    str_get(agent, "free_vram_gb", "?"),
                    str_get(agent, "total_vram_gb", "?")
                )),
                escape(&format!(
                    "{} ago",
                    format_age(agent.and_then(|a| a.get("age_seconds")))
                )),
            ];
            agent_rows.push_str("<tr>");
            for value in values {
                agent_rows.push_str(&format!("<td>{value}</td>"));
            }
            agent_rows.push_str("</tr>");
        }
    }

    let mut failed_rows = String::new();
    if let Some(rows) = state.get("recent_failed").and_then(Value::as_array) {
        for row in rows {
            let row = as_obj(Some(row));
            failed_rows.push_str("<tr>");
            for value in [
                e_get(row, "job_id", "None"),
                e_get(row, "model", "None"),
                e_get(row, "task", "None"),
                escape("details withheld"),
            ] {
                failed_rows.push_str(&format!("<td>{value}</td>"));
            }
            failed_rows.push_str("</tr>");
        }
    }

    let mut completed_rows = String::new();
    if let Some(rows) = state.get("completed_recent").and_then(Value::as_array) {
        for row in rows {
            let row = as_obj(Some(row));
            let values = [
                e_get(row, "job_id", "None"),
                e_get(row, "model", "None"),
                e_get(row, "task", "None"),
                escape(&format_age(row.and_then(|r| r.get("wall_seconds")))),
                e_or(row.and_then(|r| r.get("completed_at")), "?"),
            ];
            completed_rows.push_str("<tr>");
            for value in values {
                completed_rows.push_str(&format!("<td>{value}</td>"));
            }
            completed_rows.push_str("</tr>");
        }
    }

    let mut artifact_rows = String::new();
    if let Some(artifacts) = state.get("artifacts").and_then(Value::as_array) {
        for artifact in artifacts {
            let artifact = as_obj(Some(artifact));
            let summary = as_obj(artifact.and_then(|a| a.get("summary")));
            let raw_coverage = format!(
                "{}/{}",
                str_get(summary, "raw_leaves_complete", "?"),
                str_get(summary, "raw_leaves_expected", "?")
            );
            let aggregated_coverage = format!(
                "{}/{}",
                str_get(summary, "aggregated_leaves_complete", "?"),
                str_get(summary, "aggregated_leaves_expected", "?")
            );
            let ref_text = artifact
                .and_then(|a| a.get("ref"))
                .filter(|v| py_truthy(v))
                .map(py_str)
                .unwrap_or_default();
            let detail_url = format!(
                "/api/artifact.json?ref={}",
                crate::queue::gcs::percent_encode(&ref_text)
            );
            let aliases = artifact
                .and_then(|a| a.get("aliases"))
                .and_then(Value::as_array)
                .map(|aliases| aliases.iter().map(py_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            artifact_rows.push_str("<tr>");
            artifact_rows.push_str(&format!(
                "<td><a href=\"{}\"><code>{}</code></a><br><span class=\"muted\">{}</span></td>",
                e(&Value::String(detail_url)),
                e(&Value::String(ref_text)),
                e_or(artifact.and_then(|a| a.get("title")), ""),
            ));
            artifact_rows.push_str(&format!(
                "<td>{}</td>",
                if aliases.is_empty() {
                    escape("-")
                } else {
                    escape(&aliases)
                }
            ));
            artifact_rows.push_str(&format!(
                "<td>{}</td>",
                e_or(artifact.and_then(|a| a.get("verification")), "-")
            ));
            artifact_rows.push_str(&format!(
                "<td>raw {}<br>aggregated {}</td>",
                escape(&raw_coverage),
                escape(&aggregated_coverage)
            ));
            artifact_rows.push_str(&format!(
                "<td>{}</td>",
                e_or(artifact.and_then(|a| a.get("run_id")), "-")
            ));
            artifact_rows.push_str(&format!(
                "<td><code>{}</code></td>",
                e_or(artifact.and_then(|a| a.get("primary_uri")), "")
            ));
            artifact_rows.push_str("</tr>");
        }
    }

    let refresh_ms = refresh.max(1) * 1000;
    let stale_count = state
        .get("stale_agents")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    let mut out = String::new();
    out.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>Stado Control Center</title>\n<style>\n:root{color-scheme:light dark}body{font-family:-apple-system,system-ui,sans-serif;margin:1em;color:#222;background:#fff}\nh2{margin-top:1.2em;border-bottom:1px solid #ddd;padding-bottom:.3em}table{border-collapse:collapse;font-size:13px;margin:.4em 0;max-width:100%;display:block;overflow-x:auto}\nth,td{border:1px solid #ddd;padding:4px 8px;text-align:left;vertical-align:top}th{background:#f5f5f5}\n.big{font-size:24px;font-weight:600}.muted{color:#666;font-size:12px}.warn{color:#a00}\n.cleanup-card{margin:1.2em 0;padding:1em;border:1px solid #ccc;border-radius:12px;background:#fafafa}.cleanup-heading{display:flex;gap:1em;align-items:flex-start;justify-content:space-between}.cleanup-heading h2{margin:0;border:0}.cleanup-heading p{margin:.3em 0}.controls{display:flex;gap:.5em;flex-wrap:wrap}button{font:inherit;padding:.5em .8em;cursor:pointer}button:disabled{cursor:wait;opacity:.6}.service-status{font-weight:600}.cleanup-grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:.8em;margin:1em 0 0}.cleanup-grid div{min-width:0}dt{font-size:12px;color:#666}dd{margin:.2em 0 0;font-weight:600;overflow-wrap:anywhere}\n@media(max-width:800px){.cleanup-grid{grid-template-columns:repeat(2,minmax(0,1fr))}}@media(max-width:480px){.cleanup-heading{display:block}.controls{margin-top:.8em}.cleanup-grid{grid-template-columns:1fr}}\n@media(prefers-color-scheme:dark){body{color:#eee;background:#111}th{background:#222}.cleanup-card{background:#181818;border-color:#444}.muted,dt{color:#aaa}}\n</style></head><body>\n<h1>Stado Control Center</h1><div class=\"muted\">refreshed every ");
    out.push_str(&e(&Value::from(refresh)));
    out.push_str("s &middot; now ");
    out.push_str(&e_or(state.get("now"), "?"));
    out.push_str("</div>\n");
    out.push_str(&cleanup_card(cleanup));
    out.push('\n');
    out.push_str(policy_card());
    out.push_str(operator_workspace());
    out.push_str("\n<h2>autonomy &amp; FinOps</h2><div class=\"cleanup-card\">");
    out.push_str("<div>mode <strong>");
    out.push_str(&e_get(autonomy, "mode", "report"));
    out.push_str("</strong> &middot; emergency pause <strong>");
    out.push_str(&e_get(autonomy, "emergency_paused", "false"));
    out.push_str("</strong> &middot; circuit breaker <strong>");
    out.push_str(&e_get(autonomy, "circuit_open", "false"));
    out.push_str("</strong> &middot; mutation failures <strong>");
    out.push_str(&e_get(autonomy, "consecutive_mutation_failures", "none"));
    out.push_str("</strong> &middot; decisions <strong>");
    out.push_str(&e_get(autonomy, "decisions", "0"));
    out.push_str("</strong></div><div>current burn <strong>$");
    out.push_str(&e_get(forecast, "current_hourly_usd", "0"));
    out.push_str("/h</strong> &middot; month-end <strong>$");
    out.push_str(&e_get(forecast, "end_of_month_usd", "0"));
    out.push_str("</strong> &middot; projected overrun <strong>$");
    out.push_str(&e_get(forecast, "projected_overrun_usd", "0"));
    out.push_str("</strong></div><div>predicted savings <strong>$");
    out.push_str(&e_get(savings, "predicted_savings_usd", "0"));
    out.push_str("</strong> &middot; realized savings <strong>$");
    out.push_str(&e_get(savings, "realized_savings_usd", "0"));
    out.push_str("</strong> &middot; active anomalies <strong>");
    out.push_str(&anomaly_count.to_string());
    out.push_str("</strong></div><div class=\"muted\">JSON: <a href=\"/api/state.json\">/api/state.json</a>; control with <code>stado optimize</code> and <code>stado cost</code>.</div></div>");
    out.push_str("\n<h2>queue</h2><div class=\"big\">queued <strong>");
    out.push_str(&e_get(counts, "queue", "0"));
    out.push_str("</strong> &nbsp; running <strong>");
    out.push_str(&e_get(counts, "running", "0"));
    out.push_str("</strong> &nbsp; completed <strong>");
    out.push_str(&e_get(counts, "completed", "0"));
    out.push_str("</strong> &nbsp; failed <strong>");
    out.push_str(&e_get(counts, "failed", "0"));
    out.push_str(
        "</strong></div>\n<h2>throughput &amp; ETA</h2><div>avg wall per completed job: <strong>",
    );
    out.push_str(&escape(&format_age(
        throughput.and_then(|t| t.get("avg_wall_seconds_per_completed_job")),
    )));
    out.push_str("</strong> (");
    out.push_str(&e_get(throughput, "samples", "0"));
    out.push_str(" samples)</div><div>live free slots across all agents: <strong>");
    out.push_str(&e_get(throughput, "live_total_free_slots", "0"));
    out.push_str("</strong></div><div>projected drain of current queue: <strong>");
    out.push_str(&escape(&format_age(
        throughput.and_then(|t| t.get("projected_remaining_seconds")),
    )));
    out.push_str("</strong></div>\n<h2>per model</h2><table><tr><th>model</th><th>queued</th><th>running</th><th>completed</th><th>failed</th></tr>");
    if model_rows.is_empty() {
        out.push_str("<tr><td colspan=\"5\" class=\"muted\">no jobs</td></tr>");
    } else {
        out.push_str(&model_rows);
    }
    out.push_str("</table>\n<h2>live agents</h2><table><tr><th>consumer_id</th><th>kind</th><th>free_slots</th><th>free/total vram (GB)</th><th>last heartbeat</th></tr>");
    if agent_rows.is_empty() {
        out.push_str("<tr><td colspan=\"5\" class=\"warn\">no live agents</td></tr>");
    } else {
        out.push_str(&agent_rows);
    }
    out.push_str("</table><div class=\"muted\">stale agents (heartbeat older than threshold): ");
    out.push_str(&stale_count.to_string());
    out.push_str("</div>\n<h2>artifacts</h2><table><tr><th>immutable ref</th><th>aliases</th><th>verification</th><th>coverage</th><th>producer run</th><th>primary location</th></tr>");
    if artifact_rows.is_empty() {
        out.push_str("<tr><td colspan=\"6\" class=\"muted\">no artifacts</td></tr>");
    } else {
        out.push_str(&artifact_rows);
    }
    out.push_str("</table><div class=\"muted\">JSON: <a href=\"/api/artifacts.json\">/api/artifacts.json</a></div>\n<h2>recent failed</h2><table><tr><th>job_id</th><th>model</th><th>task</th><th>error</th></tr>");
    if failed_rows.is_empty() {
        out.push_str("<tr><td colspan=\"4\" class=\"muted\">none recent</td></tr>");
    } else {
        out.push_str(&failed_rows);
    }
    out.push_str("</table>\n<h2>recent completed</h2><table><tr><th>job_id</th><th>model</th><th>task</th><th>wall</th><th>completed_at</th></tr>");
    if completed_rows.is_empty() {
        out.push_str("<tr><td colspan=\"5\" class=\"muted\">none recent</td></tr>");
    } else {
        out.push_str(&completed_rows);
    }
    out.push_str("</table>\n<script>\n(() => {\n  const status = document.getElementById('cleanup-status');\n  const buttons = [document.getElementById('cleanup-refresh'), document.getElementById('cleanup-run')];\n  let autoRefresh = window.setTimeout(() => document.querySelector('.operator-shell')?.contains(document.activeElement) || window.location.reload(), ");
    out.push_str(&refresh_ms.to_string());
    out.push_str(");\n  async function request(url, options) {\n    window.clearTimeout(autoRefresh);\n    buttons.forEach(button => button.disabled = true); status.textContent = options ? 'Cleanup requested; waiting for completion.' : 'Refreshing cleanup status.';\n    try {\n      const response = await fetch(url, options); const payload = await response.json();\n      const report = payload.report || {}; status.textContent = `Service ${payload.service || 'unknown'}; outcome ${report.outcome || 'unknown'}.`;\n      if (response.ok) window.location.reload();\n    } catch (_) { status.textContent = 'Cleanup service request failed safely.'; }\n    finally {\n      buttons.forEach(button => button.disabled = false);\n      autoRefresh = window.setTimeout(() => window.location.reload(), ");
    out.push_str(&refresh_ms.to_string());
    out.push_str(");\n    }\n  }\n  buttons[0].addEventListener('click', () => request('/api/cleanup.json'));\n  buttons[1].addEventListener('click', () => request('/api/cleanup/run', {method:'POST', headers:{'X-Stado-Action':'cleanup'}}));\n})();\n</script>\n");
    out.push_str(policy_script());
    out.push_str("</body></html>");
    out
}

