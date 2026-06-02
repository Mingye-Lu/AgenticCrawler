#!/usr/bin/env bash
set -euo pipefail

# acrawl installer — downloads the latest release for Linux and macOS
# Usage: curl -fsSL https://raw.githubusercontent.com/Mingye-Lu/AgenticCrawler/main/install.sh | bash

REPO="Mingye-Lu/AgenticCrawler"
INSTALL_DIR="${ACRAWL_INSTALL_DIR:-$HOME/.local/bin}"
CONFIG_HOME="${ACRAWL_CONFIG_HOME:-$HOME/.acrawl}"

# --- Check required tools ---
for cmd in curl; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Error: '$cmd' is required but not found. Please install it first." >&2
        exit 1
    fi
done

if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    echo "Error: 'sha256sum' or 'shasum' is required but neither was found." >&2
    exit 1
fi

# --- 1. Detect OS and architecture ---
os=$(uname -s)
arch=$(uname -m)

case "$os" in
    Linux)
        case "$arch" in
            x86_64)  artifact_name="acrawl-linux-x64" ;;
            aarch64) artifact_name="acrawl-linux-arm64" ;;
            *)
                echo "Error: Unsupported architecture: $arch. Only x86_64 and aarch64 are supported." >&2
                exit 1
                ;;
        esac
        ;;
    Darwin)
        case "$arch" in
            x86_64)  artifact_name="acrawl-macos-x64" ;;
            arm64)   artifact_name="acrawl-macos-arm64" ;;
            *)
                echo "Error: Unsupported architecture: $arch. Only x86_64 and arm64 are supported." >&2
                exit 1
                ;;
        esac
        ;;
    *)
        echo "Error: Unsupported operating system: $os. Only Linux and macOS are supported." >&2
        exit 1
        ;;
esac

echo "Detected: $os $arch -> $artifact_name"

# --- 2. Get latest version from GitHub API ---
echo "Fetching latest release version..."
api_response=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")
version=$(echo "$api_response" | grep '"tag_name"' | sed 's/.*"tag_name": *"//;s/".*//')

if [ -z "$version" ]; then
    echo "Error: Failed to determine latest version from GitHub API" >&2
    exit 1
fi

# Defence in depth: the sed-based extraction above can quietly produce a
# non-version string if the GitHub API response changes shape or is replaced
# with attacker-controlled content via a misconfigured proxy. Reject anything
# that isn't a recognisable semver tag before it flows into download URLs.
if ! [[ "$version" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+([-+.][A-Za-z0-9.-]+)?$ ]]; then
    echo "Error: GitHub API returned an unexpected version string: '$version'" >&2
    exit 1
fi

echo "Latest version: $version"

# --- 3. Download binary ---
download_url="https://github.com/${REPO}/releases/download/${version}/${artifact_name}"
echo "Downloading ${artifact_name} (${version})..."
curl -fsSL -o "/tmp/${artifact_name}" "$download_url"

# --- 4. Download checksums ---
checksums_url="https://github.com/${REPO}/releases/download/${version}/checksums.sha256"
curl -fsSL -o "/tmp/checksums.sha256" "$checksums_url"

# --- 5. Verify checksum ---
echo "Verifying checksum..."
if command -v sha256sum >/dev/null 2>&1; then
    (cd /tmp && grep -F "${artifact_name}" checksums.sha256 | sha256sum --check --status)
else
    (cd /tmp && grep -F "${artifact_name}" checksums.sha256 | shasum -a 256 --check --status)
fi
echo "Checksum verified."

# --- 6. Create install directory ---
mkdir -p "$INSTALL_DIR"

# --- 7. Install binary ---
mv "/tmp/${artifact_name}" "${INSTALL_DIR}/acrawl"
chmod +x "${INSTALL_DIR}/acrawl"
echo "Installed acrawl to ${INSTALL_DIR}/acrawl"

# --- 8. PATH check ---
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

# --- 9. Node.js check ---
node_version=$(node --version 2>/dev/null || true)
node_major=""
if [ -n "$node_version" ]; then
    node_major=$(echo "$node_version" | sed 's/^v//' | cut -d. -f1)
fi

if [ -z "$node_version" ]; then
    echo "WARNING: Node.js not found. Node.js 20+ is required for browser automation."
    echo "Install from https://nodejs.org/"
elif [ -n "$node_major" ] && [ "$node_major" -lt 20 ]; then
    echo "WARNING: Node.js 20+ required for browser automation, you have ${node_version}"
fi

# --- 10. CloakBrowser install ---
if [ -n "$node_major" ] && [ "$node_major" -ge 20 ]; then
    if [ -d "${CONFIG_HOME}/node_modules/cloakbrowser" ] && [ -d "${CONFIG_HOME}/node_modules/playwright-core" ]; then
        echo "CloakBrowser already installed at ${CONFIG_HOME}/node_modules/cloakbrowser (skipping)"
    else
        echo "Installing CloakBrowser..."
        mkdir -p "$CONFIG_HOME"
        if npm install --prefix "$CONFIG_HOME" cloakbrowser playwright-core; then
            echo "CloakBrowser installed."
        else
            echo "WARNING: CloakBrowser install failed. Run manually: npm install --prefix \"$CONFIG_HOME\" cloakbrowser playwright-core"
        fi
    fi

    # Install OS-level libraries required by Chromium (Linux only).
    # playwright-core ships an install-deps command that handles this per-distro.
    case "$(uname -s)" in
        Linux)
            echo "Installing system dependencies for Chromium..."
            if npx --prefix "$CONFIG_HOME" playwright-core install-deps chromium; then
                echo "System dependencies installed."
            else
                echo "WARNING: Could not install system dependencies (may need sudo)."
                echo "If the browser fails to launch, run:"
                echo "  sudo npx --prefix \"$CONFIG_HOME\" playwright-core install-deps chromium"
            fi
            ;;
    esac

    echo "Ensuring browser binary is downloaded..."
    if npx --prefix "$CONFIG_HOME" cloakbrowser install; then
        echo "Browser binary ready."
    else
        echo "WARNING: Browser binary download failed. It will be downloaded on first use."
    fi
fi

# --- 11. Success ---
echo ""
echo "acrawl ${version} installed successfully!"
echo ""
echo "Get started:"
echo "  acrawl                   # launch REPL (guides you through setup on first run)"
echo "  acrawl mcp install       # use as MCP server in Claude Code, Cursor, etc."
echo "  acrawl --help            # see all options"
echo ""
echo "Useful slash commands inside the REPL:"
echo "  /auth          configure or switch LLM providers"
echo "  /model         switch models (supports 25 providers)"
echo "  /headed        watch the browser in real time"
echo "  /extension     crawl with your real browser (existing cookies, logins, extensions)"
echo "  /help          see all commands"
echo ""
echo "Browser modes (only one active at a time):"
echo "  CloakBrowser   Stealth headless browser, works out of the box (default)"
echo "  Extension      Drives your real Chrome — keeps your cookies, logins, and extensions"
echo ""
echo "  To use extension mode:"
echo "    1. Download acrawl-extension.zip from the latest GitHub release"
echo "    2. Load as unpacked extension in Chrome/Edge (enable Developer mode)"
echo "    3. Run /extension in the REPL — acrawl now drives your real browser"
echo "  To switch back: /cloakbrowser"
echo ""
echo "Tip: If ANTHROPIC_API_KEY or OPENAI_API_KEY is already set, acrawl picks it up automatically."
echo ""
