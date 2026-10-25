require "json"
require "shellwords"

class OjoJournal < Formula
  desc "Capture and search exact agent originals in local session journals"
  homepage "https://github.com/oojuro/o-journal"
  url "https://github.com/oojuro/o-journal/releases/download/ojo-journal-v0.1.0/ojo-journal-v0.1.0-x86_64-unknown-linux-gnu.tar.gz"
  version "0.1.0"
  sha256 "7e159ff27c2c7bba411c93877cb497d8f3c69242f12c30216b71863890b55589"
  license "Apache-2.0"

  depends_on :linux
  depends_on arch: :x86_64

  def install
    bin.install "bin/ojo-journal", "bin/oojuro", "bin/oojuro-native", "bin/oojurod"
    pkgshare.install "LICENSE", "NOTICE", "Cargo.lock", "manifest.json", "licenses"
  end

  test do
    root = testpath/"journal"
    original = testpath/"original.txt"
    original.write("sensor calibration confirmed\nExact original context.\n")
    system bin/"ojo-journal", "init", "--root", root
    recorded = JSON.parse(shell_output([
      bin/"ojo-journal", "record", "--root", root,
      "--session-id", "formula-session", "--event-id", "formula-event",
      "--text-file", original,
    ].map(&:to_s).shelljoin))
    session = recorded.fetch("session")
    searched = JSON.parse(shell_output([
      bin/"ojo-journal", "search", "--root", root,
      "--session", session, "--query", "calibration",
    ].map(&:to_s).shelljoin))
    assert_equal original.read, searched.fetch("matches").first.fetch("input").fetch("text")
    system bin/"ojo-journal", "finish", "--root", root, "--session", session
    system bin/"ojo-journal", "archive", "--root", root, "--session", session
    fetched = JSON.parse(shell_output([
      bin/"ojo-journal", "get", "--root", root, "--session", session, "--id", "0",
    ].map(&:to_s).shelljoin))
    assert_equal original.read, fetched.fetch("input").fetch("text")
  end
end
