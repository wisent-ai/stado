//! The readable run name derived from a submission's commands.

/// Auto-derive a readable name from the run's commands: shared module +
/// model + the distinct --task values (or a count if many).
pub fn derive_run_name(commands: &[String]) -> String {
    let mut modules: Vec<String> = Vec::new();
    let mut models: Vec<String> = Vec::new();
    let mut tasks: Vec<String> = Vec::new();
    for command in commands {
        let toks: Vec<&str> = command.split_whitespace().collect();
        for (i, tok) in toks.iter().enumerate() {
            let next = toks.get(i + 1).copied().unwrap_or("");
            match *tok {
                "-m" => {
                    let module = next.rsplit('.').next().unwrap_or("").to_string();
                    if !modules.contains(&module) {
                        modules.push(module);
                    }
                }
                "--model" => {
                    let model = next
                        .trim_matches(['\'', '"'])
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if !models.contains(&model) {
                        models.push(model);
                    }
                }
                "--task" => tasks.push(next.to_string()),
                _ => {}
            }
        }
    }
    let mut parts: Vec<String> = Vec::new();
    // Python uses sets; with exactly one element iteration order is moot.
    if modules.len() == 1 {
        parts.push(modules[0].clone());
    }
    if models.len() == 1 {
        parts.push(models[0].clone());
    }
    // dict.fromkeys: distinct, first-seen order.
    let mut uniq: Vec<&str> = Vec::new();
    for task in &tasks {
        if !uniq.contains(&task.as_str()) {
            uniq.push(task);
        }
    }
    if (1..=3).contains(&uniq.len()) {
        parts.push(uniq.join("+"));
    } else if !uniq.is_empty() {
        parts.push(format!("{}tasks", uniq.len()));
    }
    parts.push(format!("{}jobs", commands.len()));
    parts.join(":")
}
