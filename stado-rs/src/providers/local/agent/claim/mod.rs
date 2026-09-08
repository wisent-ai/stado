//! The admission scan a tick ends in: what this host may claim right now
//! ([`queue`]), the census of one scan ([`scan`]), whether one candidate fits
//! the measured budgets ([`fit`]), and the claim that starts it ([`start`]).

pub mod fit;
pub mod queue;
pub mod scan;
pub mod start;
