#!/usr/bin/env bash
set -euo pipefail

bump_type="${1:?Usage: release.sh patch|minor|major}"

if [[ "$bump_type" != "patch" && "$bump_type" != "minor" && "$bump_type" != "major" ]]; then
    echo "Error: argument must be patch, minor, or major (got: $bump_type)"
    exit 1
fi

CARGO_VERSION=$(
    cargo metadata --no-deps --format-version 1 |
        uv run python -c 'import json, sys; print(next(package["version"] for package in json.load(sys.stdin)["packages"] if package["name"] == "tryke"))'
)
BUMP_VERSION=$(uv run bump-my-version show current_version)

if [[ "$CARGO_VERSION" != "$BUMP_VERSION" ]]; then
    echo "Error: Cargo.toml version ($CARGO_VERSION) does not match bump-my-version current_version ($BUMP_VERSION)." >&2
    echo "Update [tool.bumpversion].current_version before releasing." >&2
    exit 1
fi

NEXT_VERSION=$(uv run bump-my-version show new_version --increment "$bump_type")
NEXT_TAG="v$NEXT_VERSION"
echo "Releasing $NEXT_TAG"

uv run git-cliff --tag "$NEXT_TAG" --unreleased --prepend CHANGELOG.md

echo ""
echo "CHANGELOG.md updated. Edit it now, then press Enter to commit and release $NEXT_TAG."
echo "(Ctrl+C to abort at any time)"
read -r -p "Press Enter to continue: "

uvx prek run end-of-file-fixer markdownlint-cli2 --files CHANGELOG.md

git add CHANGELOG.md
git commit -m "docs: update changelog for $NEXT_TAG"

uv run bump-my-version bump "$bump_type"

git push origin HEAD
git push origin "$NEXT_TAG"
