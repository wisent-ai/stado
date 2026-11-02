//! Which `[target.…]` tables apply: the target's configuration as `rustc
//! --print cfg` prints it, and a `cfg(…)` expression (`all`, `any`, `not`,
//! names and `key = "value"`) or a target triple judged against it.

use std::collections::BTreeSet;
use std::process::Command;

pub struct Target {
    pub triple: String,
    /// Each line of `rustc --print cfg`: `unix`, `target_os="macos"`.
    pub cfg: BTreeSet<(String, Option<String>)>,
}

impl Target {
    pub fn host(rustc: &str) -> Result<Target, String> {
        let output = Command::new(rustc)
            .args(["-vV"])
            .output()
            .map_err(|error| format!("{rustc} -vV could not run: {error}"))?;
        let version = String::from_utf8_lossy(&output.stdout).into_owned();
        let triple = version
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .map(str::trim)
            .map(String::from)
            .ok_or_else(|| format!("{rustc} -vV names no host: {version}"))?;
        let printed = Command::new(rustc)
            .args(["--print", "cfg"])
            .output()
            .map_err(|error| format!("{rustc} --print cfg could not run: {error}"))?;
        if !printed.status.success() {
            return Err(format!("{rustc} --print cfg exited {}", printed.status));
        }
        let mut cfg = BTreeSet::new();
        for line in String::from_utf8_lossy(&printed.stdout).lines() {
            match line.split_once('=') {
                Some((key, value)) => {
                    cfg.insert((key.to_string(), Some(value.trim_matches('"').to_string())));
                }
                None => {
                    cfg.insert((line.to_string(), None));
                }
            }
        }
        Ok(Target { triple, cfg })
    }

    /// Whether a `[target.<platform>]` table applies.
    pub fn matches(&self, platform: &str) -> Result<bool, String> {
        let platform = platform.trim();
        match platform.strip_prefix("cfg(").and_then(|rest| rest.strip_suffix(')')) {
            Some(expression) => {
                let tokens = tokenize(expression)?;
                let mut at = 0;
                let value = self.expression(&tokens, &mut at)?;
                if at != tokens.len() {
                    return Err(format!("cfg({expression}) has words after its expression"));
                }
                Ok(value)
            }
            None => Ok(platform == self.triple),
        }
    }

    fn expression(&self, tokens: &[Token], at: &mut usize) -> Result<bool, String> {
        let Some(Token::Word(word)) = tokens.get(*at) else { return Err(String::from("a cfg name was expected")) };
        *at += 1;
        match word.as_str() {
            "all" | "any" | "not" if tokens.get(*at) == Some(&Token::Open) => {
                *at += 1;
                let mut values = Vec::new();
                while tokens.get(*at) != Some(&Token::Close) {
                    values.push(self.expression(tokens, at)?);
                    if tokens.get(*at) == Some(&Token::Comma) {
                        *at += 1;
                    }
                    if *at >= tokens.len() {
                        return Err(format!("{word}( never closed"));
                    }
                }
                *at += 1;
                Ok(match word.as_str() {
                    "all" => values.iter().all(|value| *value),
                    "any" => values.iter().any(|value| *value),
                    _ => match values.as_slice() {
                        [only] => !only,
                        _ => return Err(String::from("not() takes one expression")),
                    },
                })
            }
            _ => {
                if tokens.get(*at) == Some(&Token::Equals) {
                    *at += 1;
                    let Some(Token::Text(value)) = tokens.get(*at) else {
                        return Err(format!("cfg {word} = has no quoted value"));
                    };
                    *at += 1;
                    Ok(self.cfg.contains(&(word.clone(), Some(value.clone()))))
                } else {
                    Ok(self.cfg.contains(&(word.clone(), None)))
                }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Text(String),
    Open,
    Close,
    Comma,
    Equals,
}

fn tokenize(expression: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = expression.chars().peekable();
    while let Some(&next) = chars.peek() {
        match next {
            ' ' | '\t' | '\n' => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push(Token::Open);
            }
            ')' => {
                chars.next();
                tokens.push(Token::Close);
            }
            ',' => {
                chars.next();
                tokens.push(Token::Comma);
            }
            '=' => {
                chars.next();
                tokens.push(Token::Equals);
            }
            '"' => {
                chars.next();
                let mut value = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some(character) => value.push(character),
                        None => return Err(format!("a quoted cfg value never closed in {expression}")),
                    }
                }
                tokens.push(Token::Text(value));
            }
            _ => {
                let mut word = String::new();
                while let Some(&character) = chars.peek() {
                    if character.is_alphanumeric() || character == '_' {
                        word.push(character);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if word.is_empty() {
                    return Err(format!("the cfg expression {expression} holds {next}"));
                }
                tokens.push(Token::Word(word));
            }
        }
    }
    Ok(tokens)
}
