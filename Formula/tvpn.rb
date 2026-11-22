# Homebrew formula for tvpn. Rendered by packaging/render.mjs on each release
# and committed to Formula/tvpn.rb in https://github.com/AbhijeetKumar1505/T-VPN (the repo is
# its own tap):
#
#   brew tap abhijeetkumar1505/tvpn https://github.com/AbhijeetKumar1505/T-VPN
#   brew install tvpn
class Tvpn < Formula
  desc "On-demand WireGuard VPN for the terminal (client and self-hosted server)"
  homepage "https://github.com/AbhijeetKumar1505/T-VPN"
  version "0.1.1"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/AbhijeetKumar1505/T-VPN/releases/download/v0.1.1/tvpn-aarch64-apple-darwin.tar.gz"
      sha256 "0b996f05fbc4402c34033ba220a459255814533c6e26971260a8fde5b3740bbd"
    end
    on_intel do
      url "https://github.com/AbhijeetKumar1505/T-VPN/releases/download/v0.1.1/tvpn-x86_64-apple-darwin.tar.gz"
      sha256 "bd7ff5f13580bc1f9de27f088762d479562f0f511c6c5347b86cb66b2bc73b70"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/AbhijeetKumar1505/T-VPN/releases/download/v0.1.1/tvpn-aarch64-unknown-linux-musl.tar.gz"
      sha256 "f160eef937889ba6ed472b35951eb743acbc7e0cd51b216ec364b9113d1baa38"
    end
    on_intel do
      url "https://github.com/AbhijeetKumar1505/T-VPN/releases/download/v0.1.1/tvpn-x86_64-unknown-linux-musl.tar.gz"
      sha256 "94767d13b75feea3db60fe6837a4e5ab464195e321d7c2c82f178c3b550e9709"
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
