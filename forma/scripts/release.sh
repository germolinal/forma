#!/usr/bin/env bash
# Cuts a release: bumps the version of every crate in the workspace, commits
# it, and tags the commit `v<version>`. Pushing the tag is what publishes the
# release (.github/workflows/release.yml, which checks that the tag and
# forma/Cargo.toml agree), so that is asked separately, at the end.
#
#   scripts/release.sh patch|minor|major      (or: make release-patch, ...)
#
# Meant to run from an up-to-date, clean main. Anything else is warned about,
# and needs a confirmation.
set -euo pipefail

cd "$(dirname "$0")/.."

part=${1:-}
case "$part" in
patch | minor | major) ;;
*)
	echo "usage: $0 patch|minor|major" >&2
	exit 2
	;;
esac

# Questions go to the terminal, so that this works from make too. The default
# is always no.
confirm() {
	local answer
	read -r -p "$1 [y/N] " answer </dev/tty
	[[ "$answer" == "y" || "$answer" == "Y" ]]
}

warn() {
	echo "⚠️  $1"
	confirm "   Continue anyway?" || {
		echo "Nothing changed."
		exit 1
	}
}

# ---- where we are ---------------------------------------------------------------

branch=$(git rev-parse --abbrev-ref HEAD)
[[ "$branch" == "main" ]] || warn "You are on '$branch', not main."

if [[ -n "$(git status --porcelain)" ]]; then
	git status --short
	warn "The working tree is not clean: the release commit would leave these changes out, but tag a tree that differs from what you have."
fi

if git fetch --quiet origin main 2>/dev/null; then
	behind=$(git rev-list --count HEAD..origin/main)
	ahead=$(git rev-list --count origin/main..HEAD)
	[[ "$behind" == "0" ]] || warn "You are $behind commit(s) behind origin/main."
	[[ "$ahead" == "0" ]] || warn "You are $ahead commit(s) ahead of origin/main: they will be part of this release."
else
	warn "Could not fetch origin/main, so it is unknown whether you are up to date."
fi

# ---- the versions -------------------------------------------------------------

current=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
if [[ ! "$current" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
	echo "Cargo.toml's version, '$current', is not MAJOR.MINOR.PATCH: bump it by hand." >&2
	exit 1
fi
major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}
case "$part" in
patch) next="$major.$minor.$((patch + 1))" ;;
minor) next="$major.$((minor + 1)).0" ;;
major) next="$((major + 1)).0.0" ;;
esac
tag="v$next"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
	echo "The tag $tag already exists." >&2
	exit 1
fi

# Every crate in the workspace that carries the same version moves with it.
manifests=()
for manifest in Cargo.toml */Cargo.toml; do
	grep -q "^version = \"$current\"$" "$manifest" && manifests+=("$manifest")
done

echo
echo "Release $current → $next ($part)"
echo "  Bump:   ${manifests[*]}, and Cargo.lock"
echo "  Commit: Release $tag"
echo "  Tag:    $tag"
echo
confirm "Go ahead?" || {
	echo "Nothing changed."
	exit 1
}

# ---- bump, commit, tag --------------------------------------------------------

for manifest in "${manifests[@]}"; do
	# The first `version = ` line is the package's (dependencies are inline).
	sed -i.bak "1,/^version = /s/^version = \"$current\"$/version = \"$next\"/" "$manifest"
	rm "$manifest.bak"
done
# Only the workspace's own crates: no dependency changes in a release commit.
cargo update --workspace --quiet

git add "${manifests[@]}" Cargo.lock
git commit --quiet -m "Release $tag"
git tag -a "$tag" -m "Release $tag"
echo "✅ Committed and tagged $tag."

# ---- publish ------------------------------------------------------------------

echo
echo "Pushing the tag publishes the release: the CLI's binaries, the images,"
echo "the website and the docs."
if confirm "Push $branch and $tag to origin now?"; then
	git push origin "$branch" "$tag"
	echo "🚀 Pushed. Follow it with: gh run watch"
else
	echo "Not pushed. When ready:"
	echo "  git push origin $branch $tag"
	echo "To undo instead:"
	echo "  git tag -d $tag && git reset --hard HEAD~1"
fi
