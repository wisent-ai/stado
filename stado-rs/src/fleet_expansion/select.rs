//! Exact bounded subset selection; ties spend less, then use stable id order.
use super::constants::{CENTS_PER_USD, COMPARISON_TOLERANCE};
use super::economics::portfolio_payback;
use super::model::{Candidate, Portfolio};

#[derive(Default)]
struct Best {
    mask: u32,
    gain: f64,
    cost: i64,
}

struct Search<'a> {
    rows: &'a [&'a Candidate],
    costs: Vec<i64>,
    conflicts: Vec<u32>,
    best: Best,
}

#[derive(Clone, Copy, Default)]
struct State {
    index: usize,
    mask: u32,
    blocked: u32,
    cost: i64,
    gain: f64,
}

impl Search<'_> {
    fn walk(&mut self, state: State, budget: i64) {
        let State {
            index,
            mask,
            blocked,
            cost,
            gain,
        } = state;
        if index == self.rows.len() {
            if gain > self.best.gain + COMPARISON_TOLERANCE
                || ((gain - self.best.gain).abs() <= COMPARISON_TOLERANCE
                    && (cost < self.best.cost || (cost == self.best.cost && mask < self.best.mask)))
            {
                self.best = Best { mask, gain, cost };
            }
            return;
        }
        self.walk(
            State {
                index: index + 1,
                ..state
            },
            budget,
        );
        let bit = 1_u32 << index;
        let next_cost = cost + self.costs[index];
        if blocked & bit == 0 && next_cost <= budget {
            self.walk(
                State {
                    index: index + 1,
                    mask: mask | bit,
                    blocked: blocked | self.conflicts[index],
                    cost: next_cost,
                    gain: gain + self.rows[index].horizon_net_usd.unwrap_or_default(),
                },
                budget,
            );
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
    let conflicts = rows
        .iter()
        .map(|a| {
            rows.iter()
                .enumerate()
                .fold(u32::default(), |mask, (index, b)| {
                    if a.option.benefit_group == b.option.benefit_group
                        || a.option
                            .need_keys
                            .iter()
                            .any(|k| b.option.need_keys.contains(k))
                    {
                        mask | (1 << index)
                    } else {
                        mask
                    }
                })
        })
        .collect();
    let mut search = Search {
        rows: &rows,
        costs,
        conflicts,
        best: Best::default(),
    };
    search.walk(State::default(), budget_cents);
    let selected: Vec<&Candidate> = rows
        .iter()
        .enumerate()
        .filter(|(i, _)| search.best.mask & (1 << i) != 0)
        .map(|(_, row)| *row)
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
