//! The write verb's refusals: a refused install must reach the operator as
//! the installer's own sentence, and must install nothing.

use crate::fixture::{stderr, Delegation};

/// The product and the surface are both proven, and proven separately: the
/// same product refused on two different surfaces produces two different
/// sentences from the real installer, so a Stado that dropped or rewrote
/// `--surface` cannot produce the matching pair. Neither sentence exists
/// anywhere in Stado's source — it can only be relaying them.
///
/// `las` is a catalogue product with a `cli` recipe and no service or desktop
/// one, so both refusals happen at recipe lookup: no checkout is touched, no
/// host is contacted, nothing is installed.
#[test]
fn product_install_surfaces_the_external_refusal_verbatim() {
    let journey = Delegation::new();

    for (surface, sentence) in [
        ("service", "las has no service installation recipe"),
        ("desktop", "las has no desktop installation recipe"),
    ] {
        let direct = journey.installer(&["install", "las", "--surface", surface, "--json"]);
        assert_eq!(
            stderr(&direct).trim(),
            format!("Error: {sentence}"),
            "the real installer's refusal for surface {surface} has changed; re-probe it before \
             asserting a sentence"
        );

        let through_stado = journey.stado(&[
            "product",
            "install",
            "las",
            "--surface",
            surface,
            "--host",
            "fleet-probe",
            "--json",
        ]);
        assert_eq!(
            through_stado.status.code(),
            direct.status.code(),
            "stado did not carry the installer's exit status for surface {surface}: {}",
            stderr(&through_stado)
        );
        assert!(
            stderr(&through_stado).contains(sentence),
            "stado did not surface the installer's own refusal {sentence:?} for surface \
             {surface}: {}",
            stderr(&through_stado)
        );
    }

    // And the product argument itself, forwarded verbatim: a name no
    // catalogue holds is refused by the installer in its own words, quoting
    // back the exact string Stado passed on.
    let unknown = journey.stado(&[
        "product",
        "install",
        "nieistnieje",
        "--surface",
        "cli",
        "--json",
    ]);
    assert!(
        !unknown.status.success(),
        "an unknown product was not refused: {}",
        String::from_utf8_lossy(&unknown.stdout)
    );
    assert!(
        stderr(&unknown).contains("unknown Wisent product 'nieistnieje'"),
        "stado did not surface the installer's unknown-product refusal: {}",
        stderr(&unknown)
    );

    // Refused means refused: the installer records every installation under
    // `~/.stado/products`, and after three refusals there is nothing there.
    assert!(
        journey.recorded_products().is_empty(),
        "a refused install left a product record behind: {:?}",
        journey.recorded_products()
    );
}
