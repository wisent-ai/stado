//! Exact subset selection; ties spend less, then use stable id order.
//!
//! Every subset that fits the budget and holds no two options for the same
//! benefit group or need is considered, pruned only by what cannot win: a
//! branch whose gain plus every positive gain still open falls short of the
//! best found. No option count bounds the search.
use super::economics::portfolio_payback;
use crate::fleet_expansion::constants::{CENTS_PER_USD, COMPARISON_TOLERANCE};
use crate::fleet_expansion::model::{Candidate, Portfolio};

struct Best {
    chosen: Vec<bool>,
    gain: f64,
    cost: i64,
}

struct Search<'a> {
    rows: &'a [&'a Candidate],
    costs: Vec<i64>,
    gains: Vec<f64>,
    conflicts: Vec<Vec<usize>>,
    chosen: Vec<bool>,
    blocked: Vec<u32>,
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

impl Search<'_> {
    fn walk(&mut self, index: usize, cost: i64, gain: f64, budget: i64) {
        if index == self.rows.len() {
            if gain > self.best.gain + COMPARISON_TOLERANCE
                || ((gain - self.best.gain).abs() <= COMPARISON_TOLERANCE
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
        let open: f64 = (index..self.rows.len())
            .filter(|j| self.blocked[*j] == 0 && self.gains[*j] > 0.0)
            .map(|j| self.gains[j])
            .sum();
        if gain + open + COMPARISON_TOLERANCE < self.best.gain {
            return;
        }
        self.walk(index + 1, cost, gain, budget);
        let next_cost = cost + self.costs[index];
        if self.blocked[index] == 0 && next_cost <= budget {
            self.chosen[index] = true;
            for conflict in 0..self.conflicts[index].len() {
                self.blocked[self.conflicts[index][conflict]] += 1;
            }
            self.walk(index + 1, next_cost, gain + self.gains[index], budget);
            for conflict in 0..self.conflicts[index].len() {
                self.blocked[self.conflicts[index][conflict]] -= 1;
            }
            self.chosen[index] = false;
        }
    }
}

pub(crate) fn select(candidates: &[Candidate], budget_cents: i64) -> Portfolio {
    let mut rows: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| c.status == "eligible")
        .collect();
    rows.sort_by(|a, b| a.option.id.cmp(&b.option.id));
    let costs = rows
        .iter()
        .map(|r| (r.committed_cost_usd.unwrap_or_default() * CENTS_PER_USD).round() as i64)
        .collect();
    let gains = rows
        .iter()
        .map(|r| r.horizon_net_usd.unwrap_or_default())
        .collect();
    let conflicts = rows
        .iter()
        .map(|a| {
            rows.iter()
                .enumerate()
                .filter(|(_, b)| {
                    a.option.benefit_group == b.option.benefit_group
                        || a.option
                            .need_keys
                            .iter()
                            .any(|k| b.option.need_keys.contains(k))
                })
                .map(|(index, _)| index)
                .collect()
        })
        .collect();
    let mut search = Search {
        rows: &rows,
        costs,
        gains,
        conflicts,
        chosen: vec![false; rows.len()],
        blocked: vec![0; rows.len()],
        best: Best {
            chosen: vec![false; rows.len()],
            gain: 0.0,
            cost: 0,
        },
    };
    search.walk(0, 0, 0.0, budget_cents);
    let selected: Vec<&Candidate> = rows
        .iter()
        .zip(&search.best.chosen)
        .filter(|(_, chosen)| **chosen)
        .map(|(row, _)| *row)
        .collect();
    let committed = search.best.cost as f64 / CENTS_PER_USD;
    Portfolio {
        selected_ids: selected.iter().map(|r| r.option.id.clone()).collect(),
        upfront_usd: selected
            .iter()
            .map(|r| r.option.upfront_usd.unwrap_or_default())
            .sum(),
        committed_cost_usd: committed,
        remaining_budget_usd: (budget_cents - search.best.cost) as f64 / CENTS_PER_USD,
        monthly_net_usd: selected
            .iter()
            .map(|r| r.monthly_net_usd.unwrap_or_default())
            .sum(),
        horizon_net_usd: search.best.gain,
        payback_months: portfolio_payback(&selected),
        roi_pct: (committed > 0.0).then(|| search.best.gain / committed * CENTS_PER_USD),
    }
}
