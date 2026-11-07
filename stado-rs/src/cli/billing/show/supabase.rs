//! The Supabase part of `billing show`: whether the project behind each
//! declared database runs, and for one that does not, what Supabase reports
//! and the way off the provider.

use serde_json::Value;

use crate::cli::reporting::field;

pub(super) fn print_supabase(section: &Value) {
    println!(
        "Supabase: {} — {}",
        field(&section["status"]),
        field(&section["detail"])
    );
    for database in section["databases"].as_array().into_iter().flatten() {
        if database["listed"].as_bool() == Some(false) {
            println!(
                "Supabase database {}: project {} is not listed for this token; way off: {}",
                field(&database["database"]),
                field(&database["project_ref"]),
                field(&database["alternative"]),
            );
            continue;
        }
        println!(
            "Supabase database {}: project {} ({}) {} on organization {} (plan {}); way off: {}",
            field(&database["database"]),
            field(&database["project"]),
            field(&database["project_ref"]),
            field(&database["project_status"]),
            field(&database["organization"]),
            field(&database["plan"]),
            field(&database["alternative"]),
        );
    }
}
