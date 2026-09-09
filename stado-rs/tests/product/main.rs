//! `stado product` against the real `wisent-products` executable.
//!
//! Nothing is stubbed. `stado product` owns no catalogue and no installer of
//! its own — `cli::product` resolves `wisent-products` and hands it the verb —
//! so the only evidence worth having is a comparison against what that
//! executable produces on its own. Every assertion below therefore runs the
//! real binary directly and then through Stado, and compares.
//!
//! What this replaced, and why: the two tests here used to write a shell
//! script named `wisent-products` onto `PATH` that echoed its own argv back as
//! JSON, and asserted the echo. An argv echo is a thing only a stub can
//! produce, so the assertion could never fail for the reason it claimed to
//! defend — a Stado that forwarded nothing but spawned the script correctly
//! passed it. The refusals below come from the real installer's own mouth and
//! name the argument that produced them, which is the same proof, for real.
//!
//! The area is split by what each piece defends: [`fixture`] resolves the real
//! executable and isolates the data, [`catalog`] defends the read verb, and
//! [`refusals`] defends the write verb's refusals.

mod catalog;
mod fixture;
mod refusals;
