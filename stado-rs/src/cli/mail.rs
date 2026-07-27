use super::{CmdError, MailCommands};
use crate::mail::{self, GmailClient, MailAnalysis};

pub async fn dispatch(command: &MailCommands) -> Result<(), CmdError> {
    match command {
        MailCommands::Search {
            query,
            max_results,
            json,
        } => {
            let messages = load(query, *max_results).await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&messages)?);
            } else {
                print_search(&messages);
            }
            Ok(())
        }
        MailCommands::Analyze {
            query,
            max_results,
            json,
        } => {
            let messages = load(query, *max_results).await?;
            let report = mail::summarize(query, messages);
            if *json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("mail analysis: {} messages", report.message_count);
                println!("action required: {}", report.action_required_count);
                if !report.categories.is_empty() {
                    println!("categories:");
                    for (category, count) in &report.categories {
                        println!("  {category}: {count}");
                    }
                }
                if !report.amounts.is_empty() {
                    println!("amounts: {}", report.amounts.join(", "));
                }
                println!("messages:");
                print_search(&report.messages);
            }
            Ok(())
        }
    }
}

async fn load(query: &str, max_results: usize) -> Result<Vec<MailAnalysis>, CmdError> {
    let client = GmailClient::from_env()
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    client
        .analyze(query, max_results)
        .await
        .map_err(|err| CmdError::click(err.to_string()))
}

fn print_search(messages: &[MailAnalysis]) {
    if messages.is_empty() {
        println!("no messages");
        return;
    }
    for message in messages {
        let marker = if message.action_required {
            "ACTION"
        } else {
            "info"
        };
        let categories = if message.categories.is_empty() {
            "uncategorized".to_string()
        } else {
            message.categories.join(",")
        };
        println!(
            "[{marker}] {} | {} | {} | {}",
            message.date, message.from, categories, message.subject
        );
        if !message.amounts.is_empty() {
            println!("  amounts: {}", message.amounts.join(", "));
        }
        if !message.date_mentions.is_empty() {
            println!("  dates: {}", message.date_mentions.join(", "));
        }
        println!("  {}", message.gmail_url);
    }
}
