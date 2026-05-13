set shell := ["bash", "-c"]

# Release -  Usage: just release patch|minor|major
release type:
    #!/usr/bin/env bash
    set -euo pipefail
    type="{{type}}"

    if [[ "$type" != "patch" && "$type" != "minor" && "$type" != "major" ]]; then
        echo "Error: argument must be patch, minor, or major (got: $type)"
        exit 1
    fi

    NEXT=$(uv run bump-my-version show new_version --increment "$type")
    echo "Releasing $NEXT"

    uv run git-cliff --tag "$NEXT" --unreleased --prepend CHANGELOG.md

    echo ""
    echo "CHANGELOG.md updated. Edit it now, then press Enter to commit and release $NEXT."
    echo "(Ctrl+C to abort at any time)"
    read -r -p "Press Enter to continue: "

    git add CHANGELOG.md
    git commit -m "docs: update changelog for $NEXT"

    uv run bump-my-version bump "$type"

    git push
    git push origin "$NEXT"
