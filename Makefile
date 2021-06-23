# - Install development utilities with `cargo install convco && cargo install cargo-release`.
# - Commit following https://www.conventionalcommits.org/. Suggested types are `feat:`, `fix:`, `build:`, `chore:`, `ci:`, `docs:`, `style:`, `refactor:`, `perf:` and `test:`.
# - Release with `make release`.
# NB: CHANGELOG is only generated for v0.6 onwards since this is when conventional commits started being used.
.PHONY: release
release:
	cargo clippy -- -D warnings
	convco check v0.5.1..HEAD
	git tag v$$(convco version --bump)
	convco changelog -c .versionrc v0.5.1..v$$(convco version --bump) > CHANGELOG.md
	git tag -d v$$(convco version --bump)
	# replace empty sections in changelog
	perl -i -p0e 's/\n[#]+ (Features|Fixes|Other)[\s]*#/\n#/sg' CHANGELOG.md
	# cargo release will fail on uncommitted changes allowing us to
	# manually check and commit the updated CHANGELOG.
	cargo release $$(convco version --bump)
