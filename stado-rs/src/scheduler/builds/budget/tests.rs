//! What the budget does when the day rolls, when the registry disagrees with
//! the recipes, and when somebody sets a ceiling.

use super::*;

    use super::*;

    fn at(day: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(&format!("{day}T12:00:00Z"))
            .expect("a test instant")
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn an_undeclared_budget_is_the_workshops_three_builds_a_day() {
        let budget = BuildBudget::read(&json!({}), at("2026-09-21"));
        assert_eq!(budget.limit, DEFAULT_DAILY_BUILD_LIMIT);
        assert_eq!(budget.used, 0);
        assert!(budget.refusal(3, "a release build").is_none());
        let refusal = budget
            .refusal(4, "a release build")
            .expect("four exceeds three");
        assert!(refusal.contains("0 of 3"), "{refusal}");
        assert!(refusal.contains("stado queue budget --limit"), "{refusal}");
    }

    #[test]
    fn yesterdays_count_does_not_spend_todays_budget() {
        let document = json!({ "build_budget": { "day": "2026-09-20", "used": 3, "limit": 3 } });
        let budget = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(budget.used, 0, "the day rolled: {budget:?}");
        assert_eq!(budget.day, "2026-09-21");
        assert_eq!(budget.limit, 3, "the declared ceiling survives the roll");
    }

    #[test]
    fn a_spent_budget_refuses_and_names_the_reset() {
        let document = json!({ "build_budget": { "day": "2026-09-21", "used": 3, "limit": 3 } });
        let budget = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(budget.remaining(), 0);
        let refusal = budget
            .refusal(1, "a queue submission")
            .expect("nothing is left");
        assert!(refusal.contains("3 of 3"), "{refusal}");
        assert!(refusal.contains("2026-09-22T00:00:00Z"), "{refusal}");
        assert!(
            refusal.contains("a queue submission asks for 1 more"),
            "{refusal}"
        );
    }

    #[test]
    fn a_submission_is_counted_and_a_new_ceiling_keeps_the_count() {
        let mut document = json!({ "builds": [] });
        let budget = BuildBudget::read(&document, at("2026-09-21"));
        budget.record(
            &mut document,
            2,
            &["run-a".to_string(), "run-b".to_string()],
        );
        let after = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(after.used, 2);
        assert_eq!(after.remaining(), 1);

        after.with_limit(&mut document, 6);
        let raised = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(raised.limit, 6, "the ceiling changed");
        assert_eq!(raised.used, 2, "what was already built still counts");
        assert!(
            raised.already_charged("run-a") && raised.already_charged("run-b"),
            "a new ceiling forgot which builds were already paid for: {raised:?}"
        );
        assert!(
            document.get("builds").is_some(),
            "the rest of the registry document is untouched: {document}"
        );
    }

    /// One build reaches two charging paths — the client that submits it and
    /// the worker that claims it — and the day owes one charge for it. The
    /// submission's own key is what says so; without it the ceiling would
    /// refuse every second build the fleet legitimately started.
    #[test]
    fn a_submission_already_charged_is_not_charged_again() {
        let mut document = json!({ "builds": [] });
        let budget = BuildBudget::read(&document, at("2026-09-21"));
        budget.record(&mut document, 1, &["build-manual-x".to_string()]);
        let after = BuildBudget::read(&document, at("2026-09-21"));
        assert!(after.already_charged("build-manual-x"));
        assert!(!after.already_charged("build-manual-y"));
        assert_eq!(after.used, 1);
    }

    /// A counter that begins the day it ships would forgive every build the
    /// fleet already made that day — on 2026-09-21 three of them — and a
    /// lost registry write would forgive the builds it lost. The recipes'
    /// own runs say what was submitted and when, so the day has a floor
    /// nobody has to remember.
    #[test]
    fn builds_the_recipes_already_record_today_count_even_with_no_counter() {
        let document = json!({
            "builds": [
                { "name": "a", "runs": {
                    "darwin-arm64": { "at": "2026-09-21T17:23:27Z", "job_id": "job-1", "status": "running" },
                    "linux-amd64": { "at": "2026-09-20T09:00:00Z", "job_id": "job-0", "status": "succeeded" },
                } },
                { "name": "b", "runs": {
                    "darwin-arm64": { "at": "2026-09-21T18:06:13Z", "job_id": "job-2", "status": "running" },
                    "linux-amd64": { "at": "2026-09-21T18:21:31Z", "job_id": "", "status": "unclaimable" },
                } },
            ]
        });
        let budget = BuildBudget::read(&document, at("2026-09-21"));
        assert_eq!(
            budget.used, 2,
            "today's two submitted runs count; yesterday's and the unclaimable one do not: {budget:?}"
        );
        assert_eq!(budget.remaining(), 1);

        let stale = json!({
            "build_budget": { "day": "2026-09-21", "used": 1, "limit": 3 },
            "builds": document["builds"].clone(),
        });
        assert_eq!(
            BuildBudget::read(&stale, at("2026-09-21")).used,
            2,
            "a counter behind the recorded runs is raised to them, never lowered"
        );
    }
