# Homebrew formula. This repo doubles as its own tap:
#   brew tap bluevisor/terminal-cam https://github.com/bluevisor/terminal-cam
#   brew install terminal-cam
# The release workflow rewrites `url` / `sha256` on every tagged release.
class TerminalCam < Formula
  desc "Live webcam as ASCII art in your terminal"
  homepage "https://github.com/bluevisor/terminal-cam"
  license "MIT"
  head "https://github.com/bluevisor/terminal-cam.git", branch: "main"

  depends_on "rust" => :build

  on_linux do
    depends_on "llvm" => :build # libclang for nokhwa's V4L2 bindings
  end

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/terminal-cam --version")
  end
end
