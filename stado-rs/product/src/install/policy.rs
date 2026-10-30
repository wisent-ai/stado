//! The update policy `stado product sync` follows on this host: for each
//! product, whether a newer canonical revision is installed by itself
//! (`automatic`) or waits for a person (`held`), and the local hours an
//! automatic installation may run in.
//!
//! The policy is this host's own deployment configuration, kept at
//! `~/.stado/products/update-policy.json` and written by `stado product policy`.
//! A host without the file follows the product default: every product is
//! automatic at any hour, which is what `sync` did before a policy existed.

use crate::common::{atomic_json, emit, Arguments, Runtime};
use anyhow::{bail, Context, Result};
use chrono::NaiveTime;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Where this host keeps its update policy.
pub fn path(home: &Path) -> PathBuf {
    home.join(".stado/products/update-policy.json")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Automatic,
    Held,
}

impl Mode {
    fn parse(word: &str, at: &str) -> Result<Self> {
        match word {
            "automatic" => Ok(Self::Automatic),
            "held" => Ok(Self::Held),
            other => bail!(
                "{at}: mode {other:?} is not an update mode, so the policy cannot say whether \
                 the product installs by itself; write \"automatic\" or \"held\""
            ),
        }
    }
    fn word(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Held => "held",
        }
    }
}

/// Local hours, `HH:MM-HH:MM`; an end before the start runs past midnight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window {
    start: NaiveTime,
    end: NaiveTime,
}

impl Window {
    fn parse(text: &str, at: &str) -> Result<Self> {
        let wrong = || {
            format!(
                "{at}: window {text:?} is not two local times HH:MM-HH:MM, so the hours an \
                 automatic installation may run in cannot be read; write it as e.g. 01:00-05:00"
            )
        };
        let (start, end) = text.split_once('-').with_context(wrong)?;
        let start = NaiveTime::parse_from_str(start.trim(), "%H:%M").with_context(wrong)?;
        let end = NaiveTime::parse_from_str(end.trim(), "%H:%M").with_context(wrong)?;
        if start == end {
            bail!(
                "{at}: window {text:?} starts and ends at the same minute, so it is open at no \
                 time; hold the product instead, or name two different times"
            );
        }
        Ok(Self { start, end })
    }
    fn contains(self, now: NaiveTime) -> bool {
        if self.start < self.end {
            self.start <= now && now < self.end
        } else {
            now >= self.start || now < self.end
        }
    }
    fn text(self) -> String {
        format!(
            "{}-{}",
            self.start.format("%H:%M"),
            self.end.format("%H:%M")
        )
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rule {
    pub mode: Mode,
    pub window: Option<Window>,
    pub reason: Option<String>,
}

impl Rule {
    const DEFAULT: Self = Self {
        mode: Mode::Automatic,
        window: None,
        reason: None,
    };

    fn parse(value: &Value, at: &str) -> Result<Self> {
        let object = value.as_object().with_context(|| {
            format!(
                "{at} is {value}, not an object, so no update rule can be read from it; write \
                 {{\"mode\": \"automatic\"|\"held\", \"window\": \"HH:MM-HH:MM\", \"reason\": \"...\"}}"
            )
        })?;
        for key in object.keys() {
            if !["mode", "window", "reason"].contains(&key.as_str()) {
                bail!(
                    "{at} names {key:?}, which no update rule has, so the policy may mean \
                     something this Stado does not do; keep only mode, window and reason"
                );
            }
        }
        let mode = object
            .get("mode")
            .and_then(Value::as_str)
            .with_context(|| {
                format!(
                    "{at} has no mode, so whether the product installs by itself is unstated; \
                     add \"mode\": \"automatic\" or \"held\""
                )
            })?;
        let mode = Mode::parse(mode, at)?;
        let window = match object.get("window") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(Window::parse(text, at)?),
            Some(other) => bail!(
                "{at}: window {other} is not text, so its hours cannot be read; write it as \
                 \"HH:MM-HH:MM\" or remove it"
            ),
        };
        let reason = match object.get("reason") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
            Some(other) => bail!(
                "{at}: reason {other} is not a sentence, so nobody reading the policy learns why \
                 it holds or limits the product; write the reason as text or remove it"
            ),
        };
        if mode == Mode::Held && reason.is_none() {
            bail!(
                "{at} holds the product without a reason, so nobody can tell when it may be \
                 released; add \"reason\": \"why it is held\""
            );
        }
        if mode == Mode::Held && window.is_some() {
            bail!(
                "{at} holds the product and names a window, but a held product never installs \
                 by itself, so the window would mean nothing; remove the window or make it automatic"
            );
        }
        Ok(Self {
            mode,
            window,
            reason,
        })
    }

    fn value(&self) -> Value {
        let mut object = Map::new();
        object.insert("mode".into(), json!(self.mode.word()));
        if let Some(window) = self.window {
            object.insert("window".into(), json!(window.text()));
        }
        if let Some(reason) = &self.reason {
            object.insert("reason".into(), json!(reason));
        }
        Value::Object(object)
    }
}

/// What the policy decides for one product now.
pub enum Verdict {
    Install,
    Held(String),
    Deferred(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Policy {
    /// The file the policy was read from, `None` when the host has none.
    pub source: Option<PathBuf>,
    pub default: Rule,
    pub products: Vec<(String, Rule)>,
}

/// A policy or rule the operator wrote that cannot be followed: the declared
/// state refusing, which no retry changes. Its cause says what to write.
#[derive(Debug)]
pub struct InvalidPolicy;
impl std::fmt::Display for InvalidPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the update policy cannot be followed")
    }
}
impl std::error::Error for InvalidPolicy {}

impl Policy {
    pub fn load(home: &Path) -> Result<Self> {
        let file = path(home);
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    source: None,
                    default: Rule::DEFAULT,
                    products: Vec::new(),
                })
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "reading the update policy {} failed, so no product can be installed \
                         by policy; make the file readable by this account or remove it to \
                         follow the default (every product automatic)",
                        file.display()
                    )
                })
            }
        };
        Self::parse(&text, &file).context(InvalidPolicy)
    }

    fn parse(text: &str, file: &Path) -> Result<Self> {
        let at = file.display().to_string();
        let value: Value = serde_json::from_str(text).with_context(|| {
            format!(
                "the update policy {at} is not JSON, so no product can be installed by policy; \
                 rewrite it with `stado product policy set` or remove it to follow the default"
            )
        })?;
        let object = value.as_object().with_context(|| {
            format!("the update policy {at} is not an object; write {{\"default\": {{...}}, \"products\": {{...}}}}")
        })?;
        for key in object.keys() {
            if key != "default" && key != "products" {
                bail!(
                    "the update policy {at} names {key:?}, which no policy has, so it may mean \
                     something this Stado does not do; keep only default and products"
                );
            }
        }
        let default = match object.get("default") {
            None => Rule::DEFAULT,
            Some(rule) => Rule::parse(rule, &format!("{at}: default"))?,
        };
        let mut products = Vec::new();
        match object.get("products") {
            None => {}
            Some(Value::Object(rules)) => {
                for (product, rule) in rules {
                    products.push((
                        product.clone(),
                        Rule::parse(rule, &format!("{at}: products.{product}"))?,
                    ));
                }
            }
            Some(other) => bail!(
                "the update policy {at}: products is {other}, not an object of product rules; \
                 write {{\"<product>\": {{\"mode\": ...}}}}"
            ),
        }
        Ok(Self {
            source: Some(file.to_owned()),
            default,
            products,
        })
    }

    fn value(&self) -> Value {
        let products: Map<String, Value> = self
            .products
            .iter()
            .map(|(product, rule)| (product.clone(), rule.value()))
            .collect();
        json!({"default": self.default.value(), "products": products})
    }

    /// The rule `product` follows and whether it is its own or the default.
    pub fn rule(&self, product: &str) -> (&Rule, &'static str) {
        match self.products.iter().find(|(name, _)| name == product) {
            Some((_, rule)) => (rule, "product"),
            None => (&self.default, "default"),
        }
    }

    /// The rule for `product`, as `sync` rows and `status` report it.
    pub fn describe(&self, product: &str) -> Value {
        let (rule, from) = self.rule(product);
        let mut value = rule.value();
        value["from"] = json!(from);
        value["file"] = json!(self.source);
        value
    }

    pub fn verdict(&self, product: &str, now: NaiveTime) -> Verdict {
        let (rule, from) = self.rule(product);
        let source = match &self.source {
            Some(file) => format!("{} ({from} rule)", file.display()),
            None => "the product default".to_owned(),
        };
        match (rule.mode, rule.window) {
            (Mode::Held, _) => Verdict::Held(format!(
                "the update policy {source} holds {product}: {}; release it with `stado product \
                 policy set {product} --mode automatic`",
                // A held rule is refused without a reason when it is read.
                match rule.reason.as_deref() {
                    Some(reason) => reason,
                    None => "no reason recorded",
                }
            )),
            (Mode::Automatic, Some(window)) if !window.contains(now) => Verdict::Deferred(format!(
                "the update policy {source} installs {product} only between {} local time; it is \
                 {} now, so the next pass inside the window installs it",
                window.text(),
                now.format("%H:%M")
            )),
            (Mode::Automatic, _) => Verdict::Install,
        }
    }
}

/// `stado product policy show|set|remove`.
pub fn run(action: &str, arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(arguments);
    let mut policy = Policy::load(&runtime.home)?;
    let file = path(&runtime.home);
    if action != "show" {
        apply(action, &args, runtime, &mut policy, &file).context(InvalidPolicy)?;
        write(&file, &policy)?;
    }
    let mut shown = policy.value();
    shown["file"] = json!(file);
    shown["present"] = json!(policy.source.is_some() || action != "show");
    emit(&shown)?;
    Ok(libc::EXIT_SUCCESS)
}

/// The change `set` or `remove` asks for, made to `policy` in memory.
fn apply(
    action: &str,
    args: &Arguments,
    runtime: &Runtime,
    policy: &mut Policy,
    file: &Path,
) -> Result<()> {
    match action {
        "set" => {
            let rule = Rule::parse(
                &json!({
                    "mode": args.required("--mode")?,
                    "window": args.optional("--window")?,
                    "reason": args.optional("--reason")?,
                }),
                "stado product policy set",
            )?;
            match (args.positional.as_slice(), args.has("--default")) {
                ([], true) => policy.default = rule,
                ([product], false) => {
                    let document = crate::catalog::current(runtime)?;
                    crate::catalog::product(&document, product)?;
                    match policy.products.iter_mut().find(|(name, _)| name == product) {
                        Some((_, existing)) => *existing = rule,
                        None => policy.products.push((product.clone(), rule)),
                    }
                }
                _ => bail!(
                    "stado product policy set takes either one catalog product or --default, \
                     so the rule would apply to nothing or to more than one; name one of them"
                ),
            }
        }
        "remove" => {
            let [product] = args.positional.as_slice() else {
                bail!("stado product policy remove takes exactly one product, whose own rule is removed so it follows the default");
            };
            let before = policy.products.len();
            policy.products.retain(|(name, _)| name != product);
            if policy.products.len() == before {
                bail!(
                    "{product} has no rule of its own in {}, so there is nothing to remove; it \
                     already follows the default",
                    file.display()
                );
            }
        }
        other => bail!("unknown policy operation {other}"),
    }
    Ok(())
}

fn write(file: &Path, policy: &Policy) -> Result<()> {
    atomic_json(file, &policy.value()).with_context(|| {
        format!(
            "writing the update policy {} failed, so the policy is unchanged; make \
             ~/.stado/products writable by this account and run the command again",
            file.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveTime {
        NaiveTime::parse_from_str(text, "%H:%M").unwrap()
    }

    #[test]
    fn a_window_past_midnight_is_open_on_both_sides_of_it() {
        let window = Window::parse("23:00-02:00", "test").unwrap();
        assert!(window.contains(at("23:30")));
        assert!(window.contains(at("01:59")));
        assert!(!window.contains(at("02:00")));
        assert!(!window.contains(at("12:00")));
    }

    #[test]
    fn a_held_rule_without_a_reason_is_refused() {
        let error = Rule::parse(&json!({"mode": "held"}), "test").unwrap_err();
        assert!(format!("{error:#}").contains("without a reason"));
    }

    #[test]
    fn a_product_without_a_rule_follows_the_default() {
        let policy = Policy::parse(
            r#"{"default":{"mode":"held","reason":"disk"},"products":{"preferences":{"mode":"automatic"}}}"#,
            Path::new("policy.json"),
        )
        .unwrap();
        assert!(matches!(
            policy.verdict("preferences", at("10:00")),
            Verdict::Install
        ));
        assert!(matches!(
            policy.verdict("tama", at("10:00")),
            Verdict::Held(_)
        ));
    }
}
