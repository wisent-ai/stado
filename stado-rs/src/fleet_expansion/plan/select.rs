//! Exact subset selection; ties spend less, then use stable id order.
//!
//! Every subset that fits the budget and holds no two options for the same
//! benefit group or need is considered, pruned only by what cannot win: a
//! branch whose gain plus every positive gain still open falls short of the
//! best found. No option count bounds the search.
use std::collections::BTreeSet;

use super::economics::{cents, portfolio_payback, positive, priced, Priced};
use crate::fleet_expansion::constants::{CENTS_PER_USD, PER_CENT};
use crate::fleet_expansion::model::{Candidate, CandidateStatus, Portfolio};
use crate::primitives::tally::Tally;

/// The best selection found: gains and costs in whole cents, so ties are
/// exact.
struct Best {
    chosen: Vec<bool>,
    gain: i64,
    cost: i64,
}

struct Search {
    costs: Vec<i64>,
    gains: Vec<i64>,
    conflicts: Vec<Vec<usize>>,
    chosen: Vec<bool>,
    /// For each row, the chosen rows that rule it out.
    blocked: Vec<BTreeSet<usize>>,
    best: Best,
}

/// Whether selection `a` precedes `b` in stable id order: at the last row
/// where they differ, `a` leaves the row out.
fn precedes(a: &[bool], b: &[bool]) -> bool {
    a.iter()
        .zip(b)
        .rev()
        .find(|(left, right)| left != right)
        .is_some_and(|(left, _)| !left)
}

impl Search {
    /// Decide the row `left` rows before the end, and every row after it.
    fn walk(&mut self, left: usize, cost: i64, gain: i64, budget: i64) {
        let rows = self.costs.len();
        if left > rows {
            return;
        }
        let index = rows - left;
        if index == rows {
            if gain > self.best.gain
                || (gain == self.best.gain
                    && (cost < self.best.cost
                        || (cost == self.best.cost && precedes(&self.chosen, &self.best.chosen))))
            {
                self.best = Best {
                    chosen: self.chosen.clone(),
                    gain,
                    cost,
                };
            }
            return;
        }
        let open: i64 = (index..rows)
            .filter(|j| self.blocked[*j].is_empty() && self.gains[*j].is_positive())
            .map(|j| self.gains[j])
            .sum();
        if gain + open < self.best.gain {
            return;
        }
        let mut rest = left;
        rest.retract();
        self.walk(rest, cost, gain, budget);
        let next_cost = cost + self.costs[index];
        if self.blocked[index].is_empty() && next_cost <= budget {
            self.chosen[index] = true;
            for conflict in self.conflicts[index].clone() {
                self.blocked[conflict].insert(index);
            }
            self.walk(rest, next_cost, gain + self.gains[index], budget);
            for conflict in self.conflicts[index].clone() {
                self.blocked[conflict].remove(&index);
            }
            self.chosen[index] = false;
        }
    }
}

pub(crate) fn select(candidates: &[Candidate], budget_cents: i64, months: u32) -> Portfolio {
    // `candidate` marks a row eligible only when its option states every
    // assumption, so every eligible row prices.
    let mut rows: Vec<Priced<'_>> = candidates
        .iter()
        .filter(|c| c.status == CandidateStatus::Eligible)
        .filter_map(|c| priced(c, months))
        .collect();
    rows.sort_by(|a, b| a.row.option.id.cmp(&b.row.option.id));
    let costs: Vec<i64> = rows.iter().map(|r| cents(r.figures.committed)).collect();
    let gains = rows.iter().map(|r| cents(r.figures.gain)).collect();
    let conflicts = rows
        .iter()
        .map(|a| {
            rows.iter()
                .enumerate()
                .filter(|(_, b)| {
                    a.row.option.benefit_group == b.row.option.benefit_group
                        || a.row
                            .option
                            .need_keys
                            .iter()
                            .any(|k| b.row.option.need_keys.contains(k))
                })
                .map(|(index, _)| index)
                .collect()
        })
        .collect();
    // Choosing nothing gains and costs the sum of nothing; every other
    // selection is measured against it.
    let mut search = Search {
        costs,
        gains,
        conflicts,
        chosen: vec![false; rows.len()],
        blocked: rows.iter().map(|_| BTreeSet::new()).collect(),
        best: Best {
            chosen: vec![false; rows.len()],
            gain: std::iter::empty::<i64>().sum(),
            cost: std::iter::empty::<i64>().sum(),
        },
    };
    let nothing_spent = search.best.cost;
    let nothing_gained = search.best.gain;
    search.walk(rows.len(), nothing_spent, nothing_gained, budget_cents);
    let selected: Vec<&Priced<'_>> = rows
        .iter()
        .zip(&search.best.chosen)
        .filter(|(_, chosen)| **chosen)
        .map(|(row, _)| row)
        .collect();
    let committed = search.best.cost as f64 / CENTS_PER_USD;
    let horizon = search.best.gain as f64 / CENTS_PER_USD;
    Portfolio {
        selected_ids: selected.iter().map(|r| r.row.option.id.clone()).collect(),
        upfront_usd: selected.iter().map(|r| r.figures.upfront).sum(),
        committed_cost_usd: committed,
        remaining_budget_usd: (budget_cents - search.best.cost) as f64 / CENTS_PER_USD,
        monthly_net_usd: selected.iter().map(|r| r.figures.net).sum(),
        horizon_net_usd: horizon,
        payback_months: portfolio_payback(&selected),
        roi_pct: positive(committed).then(|| horizon / committed * PER_CENT),
    }
}
