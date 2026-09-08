//! The verified runtime: one component's state, the whole report, and the
//! verdicts and refusals read off it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{
    BROWSER_ENGINE_MISSING, BROWSER_ENGINE_PRESENT, BROWSER_ENGINE_UNKNOWN, COMPONENT_MISSING,
    COMPONENT_PRESENT, COMPONENT_UNKNOWN, OK_STATUS, RUNTIME_BROWSER_ENGINE_MISSING,
    RUNTIME_BROWSER_ENGINE_UNKNOWN, RUNTIME_COMPLETE, RUNTIME_INCOMPLETE, RUNTIME_UNKNOWN,
};

/// One component's state on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentState {
    pub name: String,
    pub revision: String,
    pub install_by_default: bool,
    /// The absolute path checked, as the host resolved it.
    pub expected_path: String,
    /// [`COMPONENT_PRESENT`], [`COMPONENT_MISSING`] or [`COMPONENT_UNKNOWN`].
    pub state: String,
}

/// The whole runtime, verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReport {
    pub components: Vec<ComponentState>,
    /// The components this invocation requires from the Playwright cache.
    ///
    /// This list decides `required_state`; browser-engine readiness is measured
    /// independently across Chromium, Firefox, and WebKit so satisfying a
    /// recording-only requirement can never masquerade as page readiness.
    pub required: Vec<String>,
}

impl RuntimeReport {
    /// Whether every component explicitly required by this invocation is ready.
    pub fn required_state(&self) -> &'static str {
        let mut found = false;
        let mut unknown = false;
        for component in self
            .components
            .iter()
            .filter(|component| self.required.iter().any(|name| name == &component.name))
        {
            found = true;
            if component.state == COMPONENT_MISSING {
                return RUNTIME_INCOMPLETE;
            }
            unknown |= component.state == COMPONENT_UNKNOWN;
        }
        if !found || unknown {
            RUNTIME_UNKNOWN
        } else {
            RUNTIME_COMPLETE
        }
    }

    /// Whether any Playwright Chromium, Firefox, or WebKit engine can open a page.
    pub fn browser_engine_state(&self) -> &'static str {
        let mut found = false;
        let mut unknown = false;
        for component in self
            .components
            .iter()
            .filter(|component| is_browser_engine(&component.name))
        {
            found = true;
            if component.state == COMPONENT_PRESENT {
                return BROWSER_ENGINE_PRESENT;
            }
            unknown |= component.state == COMPONENT_UNKNOWN;
        }
        if !found || unknown {
            BROWSER_ENGINE_UNKNOWN
        } else {
            BROWSER_ENGINE_MISSING
        }
    }

    /// The overall browser-task readiness shown in the report.
    pub fn verdict(&self) -> &'static str {
        match self.required_state() {
            RUNTIME_INCOMPLETE => RUNTIME_INCOMPLETE,
            RUNTIME_UNKNOWN => RUNTIME_UNKNOWN,
            _ => match self.browser_engine_state() {
                BROWSER_ENGINE_PRESENT => RUNTIME_COMPLETE,
                BROWSER_ENGINE_MISSING => RUNTIME_BROWSER_ENGINE_MISSING,
                _ => RUNTIME_BROWSER_ENGINE_UNKNOWN,
            },
        }
    }

    /// Every required component that is not there.
    pub fn missing(&self) -> Vec<&ComponentState> {
        self.components
            .iter()
            .filter(|component| {
                self.required.iter().any(|name| name == &component.name)
                    && component.state == COMPONENT_MISSING
            })
            .collect()
    }

    /// Why this host cannot open a page, or `None`.
    pub fn failure(&self, host: &str) -> Option<String> {
        match self.required_state() {
            RUNTIME_UNKNOWN => Some(format!(
                "{host}: the required Playwright components could not be judged because the \
                 release requirement or cache was unreadable"
            )),
            RUNTIME_INCOMPLETE => {
                let missing = self.missing();
                let listed = missing
                    .iter()
                    .map(|component| {
                        format!(
                            "{} {} expected at {}",
                            component.name, component.revision, component.expected_path
                        )
                    })
                    .collect::<Vec<String>>()
                    .join("; ");
                let components = missing
                    .iter()
                    .map(|component| format!("\"{}\"", component.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "{host}: the browser runtime is incomplete, so every browser task fails at \
                     `browserContext.newPage` before any navigation: {listed}; create a \
                     wisent.weles-browser-runtime-plan.v1 plan with components [{components}] and \
                     repair=true, then run `stado workload run weles-browser-runtime --target \
                     {host} --plan PLAN.json`."
                ))
            }
            _ => match self.browser_engine_state() {
                BROWSER_ENGINE_MISSING => Some(format!(
                    "{host}: required Playwright components are complete, but no Chromium, \
                     Firefox, or WebKit engine is installed, so `browserContext.newPage` cannot \
                     open a page; create a wisent.weles-browser-runtime-plan.v1 plan with \
                     components [\"chromium\"] and repair=true, then run `stado workload run \
                     weles-browser-runtime --target {host} --plan PLAN.json`."
                )),
                BROWSER_ENGINE_UNKNOWN => Some(format!(
                    "{host}: required Playwright components are complete, but browser-engine \
                     readiness could not be judged, so `browserContext.newPage` is not known to \
                     work."
                )),
                _ => None,
            },
        }
    }

    pub fn to_report(&self, target: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("host".to_string(), json!(target));
        object.insert("status".to_string(), json!(OK_STATUS));
        object.insert("runtime".to_string(), json!(self.verdict()));
        object.insert("required".to_string(), json!(self.required));
        object.insert("required_state".to_string(), json!(self.required_state()));
        object.insert(
            "browser_engine_state".to_string(),
            json!(self.browser_engine_state()),
        );
        object.insert(
            "components".to_string(),
            serde_json::to_value(&self.components).unwrap_or(Value::Null),
        );
        object
    }
}

fn is_browser_engine(name: &str) -> bool {
    name == "webkit" || name.starts_with("chromium") || name.starts_with("firefox")
}
