use crate::release_pipeline::{BuildReceipt, PlatformRecipe, StepStatus};

/// Explain the first unmet proof obligation using the declared command and
/// the worker's actual observation. A passing build alone proves no behavior.
pub(super) fn incomplete(recipe: &PlatformRecipe, receipt: &BuildReceipt) -> Option<String> {
    if receipt.build.status != StepStatus::Passed
        || receipt.build.exit_code != Some(nix::libc::EXIT_SUCCESS)
    {
        return Some(format!(
            "build command {:?} has status {:?} and exit {:?}; passing build evidence is required before behavioral qualification",
            receipt.build.argv, receipt.build.status, receipt.build.exit_code
        ));
    }
    if recipe.tests.is_empty() {
        return Some("manifest declares no post-build tests; a passing build is not behavioral qualification".into());
    }
    for test in &recipe.tests {
        let matches = |step: &&crate::release_pipeline::StepReceipt| {
            step.name.strip_prefix("test:") == Some(test.name.as_str())
        };
        let mut recorded = receipt.quality.iter().filter(matches);
        let Some(step) = recorded.next() else {
            return Some(format!(
                "declared post-build test {:?} with command {:?} has no receipt entry",
                test.name, test.argv
            ));
        };
        if recorded.next().is_some() {
            return Some(format!(
                "declared post-build test {:?} has {} receipt entries; its execution is ambiguous",
                test.name,
                receipt.quality.iter().filter(matches).count()
            ));
        }
        if step.argv != test.argv {
            return Some(format!(
                "post-build test {:?} recorded command {:?}, but the manifest declares {:?}",
                test.name, step.argv, test.argv
            ));
        }
        if step.status != StepStatus::Passed || step.exit_code != Some(nix::libc::EXIT_SUCCESS) {
            return Some(format!(
                "post-build test {:?} with command {:?} has status {:?} and exit {:?}; passing test evidence is required",
                test.name, step.argv, step.status, step.exit_code
            ));
        }
    }
    None
}
