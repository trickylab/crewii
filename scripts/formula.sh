#!/bin/sh
# Usage: scripts/formula.sh <tag> <dir with crewii-<tag>-<target>.tar.gz.sha256 files>
# Prints the Homebrew formula for the release to stdout.
set -eu
tag=$1 dir=$2
sha() { cut -d' ' -f1 "$dir/crewii-$tag-$1.tar.gz.sha256"; }
url() { echo "https://github.com/trickylab/crewii/releases/download/$tag/crewii-$tag-$1.tar.gz"; }
block() {
  printf '    on_%s do\n      url "%s"\n      sha256 "%s"\n    end\n' "$1" "$(url "$2")" "$(sha "$2")"
}
cat <<EOF
class Crewii < Formula
  desc "Role-based engineering crew for OpenCode"
  homepage "https://github.com/trickylab/crewii"
  version "${tag#v}"
  license "MIT"

  on_macos do
$(block arm aarch64-apple-darwin)
$(block intel x86_64-apple-darwin)
  end

  on_linux do
$(block arm aarch64-unknown-linux-musl)
$(block intel x86_64-unknown-linux-musl)
  end

  def install
    bin.install "crewii"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/crewii --version")
  end
end
EOF
