# - Install development utilities with `cargo install convco && cargo install cargo-release`.
# - Commit following https://www.conventionalcommits.org/. Suggested types are `feat:`, `fix:`, `build:`, `chore:`, `ci:`, `docs:`, `style:`, `refactor:`, `perf:` and `test:`.
# - Release with `make release`.
# NB: CHANGELOG is only generated for v0.6 onwards since this is when conventional commits started being used.
.PHONY: release
release:
	cargo clippy -- -D warnings
	convco check v0.5.1..HEAD
	convco changelog -c .versionrc v0.5.1..HEAD > CHANGELOG.md
	# Manually commit generated CHANGELOG if it was updated
	cargo release $$(convco version --bump)
