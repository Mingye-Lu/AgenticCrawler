#!/usr/bin/env sh
# shellcheck disable=SC3040
set -euo pipefail

# acrawl installer — downloads the latest release for Linux
# Usage: curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | sh

REPO="Mingye-Lu/AgenticCrawler"
INSTALL_DIR="${ACRAWL_INSTALL_DIR:-$HOME/.local/bin}"
CONFIG_HOME="${ACRAWL_CONFIG_HOME:-$HOME/.acrawl}"

# --- Check required tools ---
for cmd in curl sha256sum; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Error: '$cmd' is required but not found. Please install it first." >&2
        exit 1
    fi
done

# --- 1. Check OS ---
os=$(uname -s)
if [ "$os" = "Darwin" ]; then
    echo "Error: macOS is not supported. See https://github.com/Mingye-Lu/AgenticCrawler for build instructions" >&2
    exit 1
fi
if [ "$os" != "Linux" ]; then
    echo "Error: Unsupported operating system: $os. Only Linux is supported." >&2
    exit 1
fi

# --- 2. Detect architecture ---
arch=$(uname -m)
case "$arch" in
    x86_64)
        artifact_name="acrawl-linux-x64"
        ;;
    aarch64)
        artifact_name="acrawl-linux-arm64"
        ;;
    *)
        echo "Error: Unsupported architecture: $arch. Only x86_64 and aarch64 are supported." >&2
        exit 1
        ;;
esac

echo "Detected: Linux $arch -> $artifact_name"

# --- 3. Get latest version from GitHub API ---
echo "Fetching latest release version..."
api_response=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")
version=$(echo "$api_response" | grep '"tag_name"' | sed 's/.*"tag_name": *"//;s/".*//')

if [ -z "$version" ]; then
    echo "Error: Failed to determine latest version from GitHub API" >&2
    exit 1
fi

echo "Latest version: $version"

# --- 4. Download binary ---
download_url="https://github.com/${REPO}/releases/download/${version}/${artifact_name}"
echo "Downloading ${artifact_name} (${version})..."
curl -fsSL -o "/tmp/${artifact_name}" "$download_url"

# --- 5. Download checksums ---
checksums_url="https://github.com/${REPO}/releases/download/${version}/checksums.sha256"
curl -fsSL -o "/tmp/checksums.sha256" "$checksums_url"

# --- 6. Verify checksum ---
echo "Verifying checksum..."
(cd /tmp && grep -F "${artifact_name}" checksums.sha256 | sha256sum --check --status)
echo "Checksum verified."

# --- 7. Create install directory ---
mkdir -p "$INSTALL_DIR"

# --- 8. Install binary ---
mv "/tmp/${artifact_name}" "${INSTALL_DIR}/acrawl"
chmod +x "${INSTALL_DIR}/acrawl"
echo "Installed acrawl to ${INSTALL_DIR}/acrawl"

# --- 9. PATH check ---
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*)
        ;;
    *)
        echo ""
        echo "WARNING: ${INSTALL_DIR} is not in your PATH."
        echo "Add it by running one of the following:"
        echo ""
        current_shell=$(basename "${SHELL:-/bin/sh}")
        case "$current_shell" in
            bash)
                echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc && source ~/.bashrc"
                ;;
            zsh)
                echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc && source ~/.zshrc"
                ;;
            fish)
                echo "  fish_add_path ${INSTALL_DIR}"
                ;;
            *)
                echo "  For bash: echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc"
                echo "  For zsh:  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc"
                echo "  For fish: fish_add_path ${INSTALL_DIR}"
                ;;
        esac
        echo ""
        ;;
esac

# --- 10. Node.js check ---
node_version=$(node --version 2>/dev/null || true)
node_major=""
if [ -n "$node_version" ]; then
    node_major=$(echo "$node_version" | sed 's/^v//' | cut -d. -f1)
fi

if [ -z "$node_version" ]; then
    echo "WARNING: Node.js not found. Node.js 16+ is required for browser automation."
    echo "Install from https://nodejs.org/"
elif [ -n "$node_major" ] && [ "$node_major" -lt 16 ]; then
    echo "WARNING: Node.js 16+ required for browser automation, you have ${node_version}"
fi

# --- 11. Playwright install ---
if [ -n "$node_major" ] && [ "$node_major" -ge 16 ]; then
    if [ -d "${CONFIG_HOME}/node_modules/playwright" ]; then
        echo "Playwright already installed at ${CONFIG_HOME}/node_modules/playwright (skipping)"
    else
        echo "Installing Playwright..."
        mkdir -p "$CONFIG_HOME"
        if npm install --prefix "$CONFIG_HOME" playwright; then
            echo "Installing Chromium browser..."
            npx --prefix "$CONFIG_HOME" playwright install chromium || echo "WARNING: Chromium install failed. Run manually: npx --prefix \"$CONFIG_HOME\" playwright install chromium"
        else
            echo "WARNING: Playwright install failed. Run manually: npm install --prefix \"$CONFIG_HOME\" playwright"
        fi
    fi
fi

# --- 12. Success ---
echo ""
echo "acrawl ${version} installed successfully!"
echo ""
echo "Next steps:"
echo "  1. Configure your LLM provider: acrawl auth anthropic"
echo "  2. Start crawling: acrawl prompt \"scrape titles from example.com\""
echo ""
