#!/usr/bin/env bash
# Publishes the `agent-usage` CLI to the public Homebrew tap. The binary is an asset of a `vX.Y.Z`
# release on this repo, and Formula/agent-usage.rb on the tap points at it. Builds for this Mac
# (Apple Silicon only so far); the menu bar app is not part of the formula.
#
#   scripts/brew-release.sh   (clean tree, HEAD pushed; the version is the workspace
#                              Cargo.toml's, and each release needs a new one)
set -euo pipefail
TAP=Fuabioo/homebrew-tap
SRC=Fuabioo/agent-usage-tray
REPO=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO"
[ -z "$(git status --porcelain)" ] || { echo "brew-release: uncommitted changes" >&2; exit 1; }
REV=$(git rev-parse HEAD)
git fetch -q origin
git merge-base --is-ancestor "$REV" origin/main || {
  echo "brew-release: HEAD is not on origin/main; push it first" >&2
  exit 1
}
VER=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
TAG=v$VER
case "$(uname -sm)" in
  "Darwin arm64") ARCH=darwin-arm64 ;;
  *) echo "brew-release: builds for Apple Silicon only so far" >&2; exit 1 ;;
esac
if gh release view "$TAG" --repo "$SRC" > /dev/null 2>&1; then
  echo "brew-release: $TAG is already on $SRC; bump the version in Cargo.toml" >&2
  exit 1
fi
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
# The binary is public: no home directory in its embedded source paths.
RUSTFLAGS="--remap-path-prefix=$HOME=~" CARGO_TARGET_DIR=$T/target \
  cargo build --release --locked -p agent-usage-cli
BIN=$T/target/release/agent-usage
if strings "$BIN" | grep -qF "$HOME"; then
  echo "brew-release: the binary embeds $HOME" >&2
  exit 1
fi
[ "$("$BIN" --version)" = "agent-usage $VER" ] || { echo "brew-release: version mismatch" >&2; exit 1; }
TGZ=agent-usage-$VER-$ARCH.tar.gz
mkdir "$T/pkg" && cp "$BIN" "$T/pkg/agent-usage" && tar -C "$T/pkg" -czf "$T/$TGZ" agent-usage
SHA=$(shasum -a 256 "$T/$TGZ" | cut -d' ' -f1)
gh release create "$TAG" --repo "$SRC" --target "$REV" --title "agent-usage $VER" \
  --notes "agent-usage CLI $VER, built from commit $REV. Install: \`brew tap fuabioo/tap && brew trust --formula fuabioo/tap/agent-usage && brew install fuabioo/tap/agent-usage\`." "$T/$TGZ"
gh repo clone "$TAP" "$T/tap" -- -q
cat > "$T/tap/Formula/agent-usage.rb" <<EOF
# Written by scripts/brew-release.sh in the agent-usage-tray repo; edit it there.
class AgentUsage < Formula
  desc "Usage and pace of AI coding agents (Claude, Codex, Charm Hyper) as JSON"
  homepage "https://github.com/$SRC"
  url "https://github.com/$SRC/releases/download/$TAG/$TGZ"
  version "$VER"
  sha256 "$SHA"
  license "MIT"

  depends_on arch: :arm64
  depends_on :macos

  def install
    bin.install "agent-usage"
  end

  def caveats
    <<~EOS
      \`agent-usage all --json\` reads every agent it finds credentials for;
      \`agent-usage claude --status\` is the human-readable view of one.
      Settings live in ~/.config/agent-usage/config.json (\`agent-usage config\`).
    EOS
  end

  test do
    assert_equal "agent-usage #{version}\\n", shell_output("#{bin}/agent-usage --version")
  end
end
EOF
if ! grep -q '^### agent-usage$' "$T/tap/README.md"; then
  python3 - "$T/tap/README.md" <<'EOF'
import sys
p = sys.argv[1]
s = open(p).read()
entry = """### agent-usage
Usage and pace of AI coding agents (Claude, Codex, Charm Hyper) as JSON, the CLI
behind the agent-usage-tray menu bar app. Apple Silicon.

```bash
brew tap fuabioo/tap && brew trust --formula fuabioo/tap/agent-usage
brew install fuabioo/tap/agent-usage
```

"""
s = s.replace("## Development\n", entry + "## Development\n", 1)
open(p, "w").write(s)
EOF
fi
cd "$T/tap"
git add Formula/agent-usage.rb README.md
git commit -q -m "agent-usage $VER"
git push -q
echo "published $TAG ($REV): brew install fuabioo/tap/agent-usage"
