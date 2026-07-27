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
    map.and_then(|m| m.get(key)).map_or_else(|| default.to_string(), py_str)
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
    format!("{}h{}m", (seconds / 3600.0) as i64, ((seconds % 3600.0) / 60.0) as i64)
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
    let active_caps = if active_caps.is_empty() { "none".to_string() } else { active_caps.join(", ") };
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

// ---------------------------------------------------------------------------
// render_html
// ---------------------------------------------------------------------------

/// Python `render_html(state, cleanup, refresh)`.
pub fn render_html(state: &Value, cleanup: &Value, refresh: i64) -> String {
    let counts = as_obj(state.get("counts"));
    let throughput = as_obj(state.get("throughput"));

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
                escape(&format!("{} ago", format_age(agent.and_then(|a| a.get("age_seconds"))))),
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
                .map(|aliases| {
                    aliases.iter().map(py_str).collect::<Vec<_>>().join(", ")
                })
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
                if aliases.is_empty() { escape("-") } else { escape(&aliases) }
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
    out.push_str("\n<h2>queue</h2><div class=\"big\">queued <strong>");
    out.push_str(&e_get(counts, "queue", "0"));
    out.push_str("</strong> &nbsp; running <strong>");
    out.push_str(&e_get(counts, "running", "0"));
    out.push_str("</strong> &nbsp; completed <strong>");
    out.push_str(&e_get(counts, "completed", "0"));
    out.push_str("</strong> &nbsp; failed <strong>");
    out.push_str(&e_get(counts, "failed", "0"));
    out.push_str("</strong></div>\n<h2>throughput &amp; ETA</h2><div>avg wall per completed job: <strong>");
    out.push_str(&escape(&format_age(throughput.and_then(|t| t.get("avg_wall_seconds_per_completed_job")))));
    out.push_str("</strong> (");
    out.push_str(&e_get(throughput, "samples", "0"));
    out.push_str(" samples)</div><div>live free slots across all agents: <strong>");
    out.push_str(&e_get(throughput, "live_total_free_slots", "0"));
    out.push_str("</strong></div><div>projected drain of current queue: <strong>");
    out.push_str(&escape(&format_age(throughput.and_then(|t| t.get("projected_remaining_seconds")))));
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
    out.push_str("</table>\n<script>\n(() => {\n  const status = document.getElementById('cleanup-status');\n  const buttons = [document.getElementById('cleanup-refresh'), document.getElementById('cleanup-run')];\n  let autoRefresh = window.setTimeout(() => window.location.reload(), ");
    out.push_str(&refresh_ms.to_string());
    out.push_str(");\n  async function request(url, options) {\n    window.clearTimeout(autoRefresh);\n    buttons.forEach(button => button.disabled = true); status.textContent = options ? 'Cleanup requested; waiting for completion.' : 'Refreshing cleanup status.';\n    try {\n      const response = await fetch(url, options); const payload = await response.json();\n      const report = payload.report || {}; status.textContent = `Service ${payload.service || 'unknown'}; outcome ${report.outcome || 'unknown'}.`;\n      if (response.ok) window.location.reload();\n    } catch (_) { status.textContent = 'Cleanup service request failed safely.'; }\n    finally {\n      buttons.forEach(button => button.disabled = false);\n      autoRefresh = window.setTimeout(() => window.location.reload(), ");
    out.push_str(&refresh_ms.to_string());
    out.push_str(");\n    }\n  }\n  buttons[0].addEventListener('click', () => request('/api/cleanup.json'));\n  buttons[1].addEventListener('click', () => request('/api/cleanup/run', {method:'POST', headers:{'X-Stado-Action':'cleanup'}}));\n})();\n</script>\n");
    out.push_str(policy_script());
    out.push_str("</body></html>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fabricated_state() -> Value {
        json!({
            "ready": true,
            "now": "2026-07-26T01:00:00+00:00",
            "bucket": "test-bucket",
            "counts": {"queue": 2, "running": 1, "completed": 3, "failed": 1},
            "by_model_state": {
                "llama-8b": {"queue": 2, "running": 1, "completed": 1, "failed": 1},
                "qwen-7b": {"queue": 0, "running": 0, "completed": 2, "failed": 0},
            },
            "live_agents": [{
                "consumer_id": "local-mac<mini>",
                "kind": "local",
                "free_slots": {"nvidia-l4": 2},
                "free_vram_gb": 20,
                "total_vram_gb": 24,
                "published_at": "2026-07-26T00:59:30+00:00",
                "age_seconds": 30.0,
                "diag": {},
            }],
            "stale_agents": [{"consumer_id": "old-agent"}],
            "recent_failed": [{"job_id": "f1", "model": "llama-8b", "task": "extract", "error": "boom"}],
            "completed_recent": [{
                "job_id": "c1", "model": "qwen-7b", "task": "steer",
                "wall_seconds": 600.0, "completed_at": "2026-07-25T23:00:00+00:00",
            }],
            "artifacts": [{
                "ref": "activations/wisent/acts:2026-07-25",
                "title": "acts<title>",
                "aliases": ["latest"],
                "verification": "passed",
                "run_id": "run-1",
                "primary_uri": "gs://bucket/acts",
                "summary": {"raw_leaves_complete": 4, "raw_leaves_expected": 4},
                "created_at": "2026-07-25T00:00:00+00:00",
            }],
            "throughput": {
                "avg_wall_seconds_per_completed_job": 600.0,
                "samples": 3,
                "live_total_free_slots": 2,
                "projected_remaining_seconds": 1200.0,
            },
            "last_refresh_seconds": 1.5,
        })
    }

    fn fabricated_cleanup() -> Value {
        cleanup_envelope(&json!({
            "mode": "report",
            "outcome": "ok",
            "free_bytes_after": 536870912000_i64,
            "low_bytes": 107374182400_i64,
            "target_bytes": 214748364800_i64,
            "pressure_active": false,
            "last_success_at": "2026-07-25T00:00:00+00:00",
            "caps": {"bytes": false, "items": true, "scan": false, "deadline": false},
            "errors": [],
            "cleaners": {
                "weles_recordings": {
                    "eligible_items": 5,
                    "deleted_items": 2,
                    "actual_free_delta_bytes": 1073741824_i64,
                    "skipped": {"too_recent": 3},
                },
            },
        }))
    }

    #[test]
    fn render_contains_key_sections_and_escapes() {
        let html = render_html(&fabricated_state(), &fabricated_cleanup(), 10);
        for needle in [
            "<title>Stado Control Center</title>",
            "refreshed every 10s",
            "Stado Disk Cleanup",
            "Registry disk policy",
            "<h2>queue</h2>",
            "queued <strong>2</strong>",
            "throughput &amp; ETA",
            "10m0s",  // avg wall 600s
            "20m0s",  // projected 1200s
            "<h2>per model</h2>",
            "llama-8b",
            "<h2>live agents</h2>",
            "stale agents (heartbeat older than threshold): 1",
            "<h2>artifacts</h2>",
            "/api/artifact.json?ref=activations%2Fwisent%2Facts%3A2026-07-25",
            "<h2>recent failed</h2>",
            "<h2>recent completed</h2>",
            "Service ready; outcome ok.",
            "report / ok",
            "500.0 GiB / 100.0 GiB / 200.0 GiB",
            "window.setTimeout(() => window.location.reload(), 10000)",
        ] {
            assert!(html.contains(needle), "missing {needle:?}");
        }
        // HTML escaping: raw `<` from values must not survive.
        assert!(html.contains("local-mac&lt;mini&gt;"));
        assert!(html.contains("acts&lt;title&gt;"));
        assert!(!html.contains("local-mac<mini>"));
    }

    #[test]
    fn render_empty_state_uses_placeholder_rows() {
        let html = render_html(&json!({}), &cleanup_envelope(&json!({})), 5);
        for needle in [
            "no jobs",
            "no live agents",
            "no artifacts",
            "none recent",
            "not configured / never_run",
            "Service ready; outcome never_run.",
            "unknown / unknown / unknown",
        ] {
            assert!(html.contains(needle), "missing {needle:?}");
        }
    }

    #[test]
    fn envelope_busy_and_age_formatting() {
        let busy = cleanup_envelope(&json!({"outcome": "lock_busy"}));
        assert_eq!(busy["ok"], false);
        assert_eq!(busy["service"], "busy");
        let busy2 = cleanup_envelope(&json!({"lock_busy": true}));
        assert_eq!(busy2["service"], "busy");
        assert_eq!(cleanup_envelope(&json!({}))["service"], "ready");

        assert_eq!(format_age(Some(&json!(45))), "45s");
        assert_eq!(format_age(Some(&json!(75))), "1m15s");
        assert_eq!(format_age(Some(&json!(3600))), "1h0m");
        assert_eq!(format_age(Some(&json!(5407))), "1h30m");
        assert_eq!(format_age(Some(&json!(-5))), "0s");
        assert_eq!(format_age(None), "?");
        assert_eq!(format_age(Some(&json!("x"))), "?");
        assert_eq!(format_age(Some(&json!(true))), "?");
        assert_eq!(bytes_gib(Some(&json!(1610612736_i64))), "1.5 GiB");
        assert_eq!(bytes_gib(Some(&json!(1.5))), "unknown");
        assert_eq!(bytes_gib(None), "unknown");
    }
}
