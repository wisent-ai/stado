//! `service env-show`: what the file a launcher `.`-sources really says.
//!
//! Every case here writes the env file first and then compares the report
//! against those same bytes read back off disk — the head's byte count, the
//! VALUE cell against the value the file holds, and the file's contents after
//! the read, because a reader that rewrites what it read is worse than one
//! that cannot read at all.

use crate::{
    assert_head_matches_disk, effective_on_disk, on_disk, row, stderr, stdout, Fleet, SERVICE,
};

#[test]
fn env_show_reports_a_duplicate_key_in_file_order_and_names_the_winner() {
    let fleet = Fleet::new();
    // The shape the 2026-08-30 outage had: the value an operator wrote with
    // `env-set` sits above an `export` spelling of the same variable, which
    // `env-set`'s `^KEY=` rewrite cannot see and which wins when sourced.
    let path = fleet.env_file(
        "# nonsecret worker configuration\n\
         WC_SKARBIEC_URL=http://127.0.0.1:8895\n\
         WELES_QUEUE=default\n\
         export WC_SKARBIEC_URL=http://127.0.0.1:8785\n",
    );
    let before = on_disk(&path);

    let out = fleet.env_show(path.to_str().unwrap(), &[]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);
    assert_head_matches_disk(&text, &path);

    // Both assignments are listed, on their own lines, in file order.
    let first = text
        .find("http://127.0.0.1:8895")
        .expect("first value shown");
    let second = text
        .find("http://127.0.0.1:8785")
        .expect("second value shown");
    assert!(first < second, "assignments out of file order:\n{text}");
    // Each assignment carries its own line number and the form it was written
    // in. The `export` spelling is reported as itself rather than normalized
    // into the plain one, because that is the difference `env-set` is blind to.
    let assignments: Vec<Vec<&str>> = text
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<&str>>())
        .filter(|fields| fields.get(2) == Some(&"WC_SKARBIEC_URL"))
        .collect();
    assert_eq!(
        assignments.len(),
        2,
        "both assignments are not listed:\n{text}"
    );
    // LINE FORM KEY RESOLUTION VALUE_STATE CHARS VALUE
    let (shadowed, effective) = (&assignments[0], &assignments[1]);
    assert_eq!(shadowed[1], "assignment", "wrong form:\n{text}");
    assert_eq!(shadowed[3], "shadowed", "wrong resolution:\n{text}");
    assert_eq!(effective[1], "export", "wrong form:\n{text}");
    assert_eq!(effective[3], "effective", "wrong resolution:\n{text}");
    // The line numbers are the lines this file really has.
    let numbered: Vec<String> = before
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("WC_SKARBIEC_URL"))
        .map(|(index, _)| (index + 1).to_string())
        .collect();
    assert_eq!(
        vec![shadowed[0].to_string(), effective[0].to_string()],
        numbered,
        "the reported line numbers are not this file's:\n{text}"
    );
    // The winner named in the table is the one the file's own bytes elect.
    assert_eq!(
        effective[6],
        effective_on_disk(&path, "WC_SKARBIEC_URL"),
        "the effective row is not the file's last assignment:\n{text}"
    );
    assert!(
        text.contains("duplicates: WC_SKARBIEC_URL"),
        "the duplicate is not called out:\n{text}"
    );
    assert!(
        text.contains("the LAST assignment wins when this file is sourced"),
        "the duplicate note does not say which one wins:\n{text}"
    );
    // A reader leaves the file exactly as it found it.
    assert_eq!(on_disk(&path), before, "env-show rewrote the file it read");
}

#[test]
fn env_show_says_every_key_is_assigned_once_when_none_repeats() {
    let fleet = Fleet::new();
    let path = fleet.env_file("WELES_QUEUE=default\nWC_SKARBIEC_URL=http://127.0.0.1:8895\n");

    let out = fleet.env_show(path.to_str().unwrap(), &[]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);
    assert_head_matches_disk(&text, &path);
    assert!(
        text.contains("duplicates: none — every key is assigned exactly once"),
        "got:\n{text}"
    );
    // Both keys report the value the file holds, and the unit the registry
    // declared is the one that was read.
    assert!(
        text.contains(&format!("unit:     {SERVICE}")),
        "got:\n{text}"
    );
    for key in ["WELES_QUEUE", "WC_SKARBIEC_URL"] {
        assert!(
            row(&text, key).ends_with(&effective_on_disk(&path, key)),
            "the {key} row does not end in the file's own value:\n{text}"
        );
    }
}

#[test]
fn env_show_withholds_credentials_and_shows_endpoints_whatever_the_key_is_called() {
    let fleet = Fleet::new();
    let path = fleet.env_file(
        "WELES_API_TOKEN=super-secret-bearer-value\n\
         WELES_CREDENTIAL_SKARBIEC_URL=http://127.0.0.1:8895\n\
         WELES_DATABASE_URL=postgres://weles:hunter2@db.internal:5432/weles\n\
         WELES_API_PORT=8896\n\
         WELES_STATE_DIR=$HOME/.local/state/weles\n",
    );

    let out = fleet.env_show(path.to_str().unwrap(), &[]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);
    assert_head_matches_disk(&text, &path);

    // A credential-shaped key never puts its value on the wire, and its
    // length is reported so the operator still learns something.
    assert!(
        !text.contains("super-secret-bearer-value"),
        "a secret crossed the channel:\n{text}"
    );
    assert!(
        row(&text, "WELES_API_TOKEN").contains("redacted"),
        "the token is not marked redacted:\n{text}"
    );
    // The withheld length is the length of the value on disk, not a guess.
    let withheld = effective_on_disk(&path, "WELES_API_TOKEN");
    assert!(
        row(&text, "WELES_API_TOKEN").contains(&withheld.chars().count().to_string()),
        "the withheld value's length is not the file's:\n{text}"
    );
    // The key an operator must verify carries CREDENTIAL in its name and is
    // shown anyway, because its value is an inert endpoint.
    assert_eq!(
        row(&text, "WELES_CREDENTIAL_SKARBIEC_URL")
            .split_whitespace()
            .last(),
        Some(effective_on_disk(&path, "WELES_CREDENTIAL_SKARBIEC_URL").as_str()),
        "an endpoint was hidden behind its key name:\n{text}"
    );
    // A URL carrying userinfo is withheld even though its key names no secret.
    assert!(
        !text.contains("hunter2"),
        "a URL password crossed the channel:\n{text}"
    );
    assert!(
        row(&text, "WELES_DATABASE_URL").contains("redacted"),
        "a userinfo URL is not marked redacted:\n{text}"
    );
    assert!(
        row(&text, "WELES_API_PORT").contains(&effective_on_disk(&path, "WELES_API_PORT")),
        "a port was hidden:\n{text}"
    );
    // A reference to another variable is not a secret and is shown.
    assert!(
        row(&text, "WELES_STATE_DIR").contains("$HOME/.local/state/weles"),
        "a variable reference was hidden:\n{text}"
    );
    // Exactly the two values above are the ones that stayed on the host.
    assert!(
        text.contains("redacted: 2 value(s) never left the host"),
        "the withheld count is not reported:\n{text}"
    );
}

#[test]
fn env_show_reveals_exactly_the_one_key_named() {
    let fleet = Fleet::new();
    let path = fleet.env_file(
        "WELES_API_TOKEN=super-secret-bearer-value\n\
         WELES_OTHER_TOKEN=another-secret-value\n",
    );

    let out = fleet.env_show(path.to_str().unwrap(), &["--reveal", "WELES_API_TOKEN"]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);
    assert_head_matches_disk(&text, &path);
    // Revealed exactly, byte for byte, from the file this test wrote.
    assert_eq!(
        row(&text, "WELES_API_TOKEN").split_whitespace().last(),
        Some(effective_on_disk(&path, "WELES_API_TOKEN").as_str()),
        "the revealed key was not the file's value:\n{text}"
    );
    assert!(
        row(&text, "WELES_API_TOKEN").contains("revealed"),
        "the revealed key is not marked as such:\n{text}"
    );
    assert!(
        !text.contains("another-secret-value"),
        "--reveal opened a key it was not given:\n{text}"
    );
}

#[test]
fn env_show_reports_a_line_that_is_not_an_assignment() {
    let fleet = Fleet::new();
    // A sourced second file changes what the whole env file means, and is
    // exactly the kind of line a reader that collapsed the file into a map
    // would drop.
    let path = fleet.env_file("WELES_QUEUE=default\n. $HOME/.config/weles/extra.env\n");

    let out = fleet.env_show(path.to_str().unwrap(), &[]);
    assert!(out.status.success(), "env-show failed: {}", stderr(&out));
    let text = stdout(&out);
    assert_head_matches_disk(&text, &path);
    assert!(
        text.contains("unparsable"),
        "a non-assignment line is not reported:\n{text}"
    );
    // The line is reported as the file spells it, second line and all.
    let source = on_disk(&path);
    let line = source.lines().nth(1).unwrap();
    assert!(
        text.contains(line),
        "the non-assignment line's text is not shown:\n{text}"
    );
}
