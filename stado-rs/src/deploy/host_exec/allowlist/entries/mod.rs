//! The table's entries, in four groups laid end to end.

mod connectivity;
mod machine;
mod readiness;
mod services;

use super::ApprovedCommand;

/// The allowlist.
///
/// Declared as a slice, not an array, so adding an entry never means
/// touching a length. Ordered roughly by how often an operator reaches for
/// it while a box is misbehaving.
pub const APPROVED_COMMANDS: &[ApprovedCommand] = &ASSEMBLED;

/// The groups the entries are declared in, in table order.
///
/// Four component files rather than one array, because the table outgrew a
/// file; the order here is the order the entries had when they shared one.
const GROUPS: &[&[ApprovedCommand]] = &[
    machine::MACHINE_READS,
    readiness::READINESS_READS,
    connectivity::CONNECTIVITY_AND_SIGN_IN,
    services::SERVICE_AND_RUNTIME_READS,
];

/// How many entries the groups hold between them.
const TOTAL: usize = counted(GROUPS);

const fn counted(groups: &[&[ApprovedCommand]]) -> usize {
    let mut entries = 0;
    let mut group = 0;
    while group < groups.len() {
        entries += groups[group].len();
        group += 1;
    }
    entries
}

/// Every group end to end, so [`APPROVED_COMMANDS`] is one slice of entries
/// exactly as it was while every entry sat in one array: an entry's position
/// in the table is its position here, and nothing that reads the table has
/// to know the entries are stored in four pieces.
const ASSEMBLED: [ApprovedCommand; TOTAL] = assembled();

const fn assembled() -> [ApprovedCommand; TOTAL] {
    let mut entries = [GROUPS[0][0]; TOTAL];
    let mut at = 0;
    let mut group = 0;
    while group < GROUPS.len() {
        let entered = GROUPS[group];
        let mut index = 0;
        while index < entered.len() {
            entries[at] = entered[index];
            at += 1;
            index += 1;
        }
        group += 1;
    }
    entries
}
