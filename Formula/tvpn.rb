# Homebrew formula for tvpn. Rendered by packaging/render.mjs on each release
# and committed to Formula/tvpn.rb in https://github.com/AbhijeetKumar1505/T-VPN (the repo is
# its own tap):
#
#   brew tap abhijeetkumar1505/tvpn https://github.com/AbhijeetKumar1505/T-VPN
#   brew install tvpn
class Tvpn < Formula
  desc "On-demand WireGuard VPN for the terminal (client and self-hosted server)"
  homepage "https://github.com/AbhijeetKumar1505/T-VPN"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/AbhijeetKumar1505/T-VPN/releases/download/v0.1.0/tvpn-aarch64-apple-darwin.tar.gz"
      sha256 "5e144a63567d281acc911679cf91d28d6f378e14779687281174f9b6e65c3b83"
    end
    on_intel do
      url "https://github.com/AbhijeetKumar1505/T-VPN/releases/download/v0.1.0/tvpn-x86_64-apple-darwin.tar.gz"
      sha256 "894b9bed9fe2e5e1ca17fbb3f2846f873394defd5282e01c6aed2dc8f7acfba0"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/AbhijeetKumar1505/T-VPN/releases/download/v0.1.0/tvpn-aarch64-unknown-linux-musl.tar.gz"
      sha256 "1a4143ddebc073e33804fde5b795b99f9151447f1ad99a171c65c3d78914f8fe"
    end
    on_intel do
      url "https://github.com/AbhijeetKumar1505/T-VPN/releases/download/v0.1.0/tvpn-x86_64-unknown-linux-musl.tar.gz"
      sha256 "79ea7c46d815522111d7c173d932ef70163a71dedcf9a2dfd76844c09a878dd8"
    end
  end

  def install
    bin.install "tvpn"
    generate_completions_from_executable(bin/"tvpn", "completions")
  end

  def caveats
    <<~EOS
      Creating a tunnel needs root: `tvpn up` re-runs itself through sudo when needed.
      Get started with:
        tvpn doctor
        tvpn import ~/Downloads/your-wireguard.conf
        tvpn up
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tvpn --version")
    key = shell_output("#{bin}/tvpn genkey").strip
    assert_equal 44, key.length
  end
end
