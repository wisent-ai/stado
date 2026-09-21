# The commands this repository is checked, built and tested with.
#
# They live in a checked-in file rather than in whatever a person types, so the
# verdict here, the one a Stado recipe reaches on every commit and the one a
# release candidate is measured against come from the same words. The crate
# lives in `stado-rs/`, which is the only reason these are not bare cargo
# lines.

.PHONY: check build release test

# Does the working copy compile? The cheap question, and the one to ask after
# an edit: a build is rationed, a check is not.
check:
	cd stado-rs && cargo check

build:
	cd stado-rs && cargo build

# What a delivery installs.
release:
	cd stado-rs && cargo build --release

test:
	cd stado-rs && cargo test
