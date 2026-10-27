//! The text form of a Vast.ai readiness reading: one line per fact, in the
//! order an operator reads them, and the remedy lines last.

use super::readiness::Readiness;

pub(super) fn print_text(report: &Readiness, summary: &str) {
    println!("{summary}");
    println!("item:     {}/{}", report.item, report.field);
    println!("channel:  {}", report.channel.describe());
    if let Some(error) = &report.skarbiec_error {
        println!("skarbiec: {error}");
    }
    if let Some(host) = &report.vault_host {
        match &report.vault_item_state {
            Some(state) => println!("vault:    {host} says {state}"),
            None => println!("vault:    {host} said nothing about the item"),
        }
    }
    if let Some(error) = &report.vault_error {
        println!("vault:    {error}");
    }
    if let Some(machine) = &report.machine_id {
        println!("machine:  {machine}");
    }
    if let Some(price) = report.listed_gpu_cost {
        println!("listed:   ${price}/h");
    }
    for line in &report.remedy {
        println!("next:     {line}");
    }
}
