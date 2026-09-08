//! The vocabulary one host's runtime is reported in, and the verdict that
//! vocabulary adds up to.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::targets::{ComputeTarget, MobileRuntime};

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "mobile_runtime";

/// The component is on the host at one of its declared paths.
pub const COMPONENT_PRESENT: &str = "present";
/// The component is not at any path this fleet installs it at.
pub const COMPONENT_MISSING: &str = "missing";
/// The component is present but reports a different version than declared.
pub const COMPONENT_DRIFTED: &str = "drifted";
/// The host could not be asked.
pub const COMPONENT_UNKNOWN: &str = "unknown";

/// Every declared component is present at its declared version.
pub const RUNTIME_COMPLETE: &str = "complete";
/// At least one is missing or drifted.
pub const RUNTIME_INCOMPLETE: &str = "incomplete";
/// The requirement or the host could not be read.
pub const RUNTIME_UNKNOWN: &str = "unknown";

/// A driver the host carries, that this declaration says nothing about, and
/// that the installed server itself calls incompatible with it.
///
/// Reported, counted and visible, and it does NOT decide the verdict. That is
/// the split [`crate::host_software`] already argues for: failing a host over
/// a program nothing declares is how an operator learns to write `|| true`
/// after the command, at which point the drift the check exists to catch
/// stops being noticed. But leaving it out of the report entirely is how
/// `charless-mac-mini` kept a `mac2@1.20.5` that the server calls
/// incompatible, ready to deadlock npm for whichever install came next.
pub const COMPONENT_UNDECLARED_INCOMPATIBLE: &str = "incompatible-undeclared";

/// One component of the runtime, as the host reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentState {
    /// `appium`, `adb`, or `driver:<name>`.
    pub name: String,
    /// What the declaration asks for: a version, or `required`.
    pub declared: String,
    /// Absolute path the host resolved it at, or the candidate list it tried.
    pub path: String,
    /// The version the program itself reported, when it ran.
    pub observed: String,
    /// [`COMPONENT_PRESENT`], `_MISSING`, `_DRIFTED` or `_UNKNOWN`.
    pub state: String,
}

/// Every component of one host's declared mobile runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReport {
    pub components: Vec<ComponentState>,
}

impl RuntimeReport {
    /// Only components this host DECLARED decide anything.
    ///
    /// An undeclared driver the server calls incompatible is in the report
    /// and not in the gate, for the reason
    /// [`COMPONENT_UNDECLARED_INCOMPATIBLE`] gives.
    fn judged(&self) -> impl Iterator<Item = &ComponentState> {
        self.components
            .iter()
            .filter(|component| component.state != COMPONENT_UNDECLARED_INCOMPATIBLE)
    }

    /// `complete` only when every declared component is present at its
    /// declaration.
    pub fn verdict(&self) -> &'static str {
        if self.judged().next().is_none() {
            return RUNTIME_UNKNOWN;
        }
        if self
            .judged()
            .any(|component| component.state == COMPONENT_UNKNOWN)
        {
            return RUNTIME_UNKNOWN;
        }
        if self
            .judged()
            .all(|component| component.state == COMPONENT_PRESENT)
        {
            return RUNTIME_COMPLETE;
        }
        RUNTIME_INCOMPLETE
    }

    /// Components a repair would have to act on.
    pub fn incomplete(&self) -> Vec<&ComponentState> {
        self.judged()
            .filter(|component| component.state != COMPONENT_PRESENT)
            .collect()
    }

    /// One sentence naming the host and the exact disagreement, or `None`
    /// when the runtime is complete. Silence is never rounded to agreement:
    /// an unknown component fails here exactly as a missing one does.
    pub fn failure(&self, host: &str) -> Option<String> {
        let broken = self.incomplete();
        if broken.is_empty() {
            return None;
        }
        Some(format!(
            "{host}: mobile runtime {} — {}",
            self.verdict(),
            broken
                .iter()
                .map(|component| format!(
                    "{} is {} (declared {}, looked at {})",
                    component.name, component.state, component.declared, component.path
                ))
                .collect::<Vec<String>>()
                .join("; ")
        ))
    }

    /// The `--json` document.
    pub fn to_report(&self, target: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("status".to_string(), json!(OK_STATUS));
        object.insert("target".to_string(), json!(target));
        object.insert("runtime".to_string(), json!(self.verdict()));
        object.insert("components".to_string(), json!(self.components));
        object
    }
}

/// The requirement this host declares, or `None` when it declares none.
///
/// Absence is not a failure and must not be rounded into one: a host that is
/// not a mobile placement is the default, and judging every host against a
/// runtime only two of them need is how an operator learns to ignore the
/// report.
pub fn requirement(target: &ComputeTarget) -> Option<&MobileRuntime> {
    target.mobile_runtime.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(name: &str, state: &str) -> ComponentState {
        ComponentState {
            name: name.to_string(),
            declared: "required".to_string(),
            path: "/x".to_string(),
            observed: String::new(),
            state: state.to_string(),
        }
    }

    #[test]
    fn a_missing_component_makes_the_runtime_incomplete_and_names_itself() {
        let report = RuntimeReport {
            components: vec![
                component("appium", COMPONENT_PRESENT),
                component("adb", COMPONENT_MISSING),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_INCOMPLETE);
        let failure = report.failure("lukasz-macbook").expect("a failure");
        assert!(failure.contains("lukasz-macbook"));
        assert!(failure.contains("adb is missing"));
        // The component that was fine is not named as a fault.
        assert!(!failure.contains("appium is"));
    }

    #[test]
    fn silence_from_one_component_is_never_rounded_to_agreement() {
        let report = RuntimeReport {
            components: vec![
                component("appium", COMPONENT_PRESENT),
                component("adb", COMPONENT_UNKNOWN),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_UNKNOWN);
        assert!(report.failure("h").is_some());
    }

    #[test]
    fn a_complete_runtime_has_no_failure() {
        let report = RuntimeReport {
            components: vec![
                component("appium", COMPONENT_PRESENT),
                component("driver:xcuitest", COMPONENT_PRESENT),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_COMPLETE);
        assert_eq!(report.failure("h"), None);
    }

    #[test]
    fn an_empty_report_is_unknown_rather_than_complete() {
        let report = RuntimeReport { components: vec![] };
        assert_eq!(report.verdict(), RUNTIME_UNKNOWN);
    }

    #[test]
    fn an_undeclared_incompatible_driver_is_reported_and_does_not_fail_the_gate() {
        // The exact mini shape: everything declared is present, and the
        // server complains about a driver the declaration never mentions.
        let report = RuntimeReport {
            components: vec![
                component("appium", COMPONENT_PRESENT),
                component("driver:uiautomator2", COMPONENT_PRESENT),
                component("adb", COMPONENT_PRESENT),
                component("driver:mac2", COMPONENT_UNDECLARED_INCOMPATIBLE),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_COMPLETE);
        assert_eq!(report.failure("charless-mac-mini"), None);
        // Still visible: it is in the report an operator reads.
        assert!(report
            .components
            .iter()
            .any(|component| component.name == "driver:mac2"));
    }

    #[test]
    fn a_declared_driver_is_still_judged_even_beside_an_undeclared_one() {
        let report = RuntimeReport {
            components: vec![
                component("driver:uiautomator2", COMPONENT_MISSING),
                component("driver:mac2", COMPONENT_UNDECLARED_INCOMPATIBLE),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_INCOMPLETE);
        let said = report.failure("h").expect("a failure");
        assert!(said.contains("uiautomator2"));
        assert!(!said.contains("mac2"));
    }

    #[test]
    fn a_host_that_declares_nothing_is_not_judged() {
        let target: ComputeTarget =
            serde_json::from_value(serde_json::json!({"name":"h","kind":"local"}))
                .expect("a minimal target");
        assert!(requirement(&target).is_none());
    }
}
