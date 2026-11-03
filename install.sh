#!/bin/sh
# Installs capsa tools from the newest release of each tool.
# Usage: install.sh [acus|pons|census|vicarius|all ...]   (default: all)
#        install.sh --uninstall <acus|pons|census|vicarius|all>
# Env: CAPSA_INSTALL_DIR (default ~/.local/bin), CAPSA_INSTALL_SCOPE=user|system,
#      CAPSA_NO_SKILL=1 skips `acus self skill --install` (ACUS_NO_SKILL works too) and
#      keeps the acus skill on --uninstall,
#      GITHUB_TOKEN (else `gh auth token`), sent when available to lift the API rate limit.
set -eu

REPO="LuMiSxh/capsa"
API="https://api.github.com/repos/${REPO}"
INSTALL_DIR="${CAPSA_INSTALL_DIR:-$HOME/.local/bin}"
INSTALL_SCOPE="${CAPSA_INSTALL_SCOPE:-user}"
NO_SKILL="${CAPSA_NO_SKILL:-${ACUS_NO_SKILL:-}}"
TOKEN=""

main() {
    uninstall=""
    for arg in "$@"; do
        if [ "$arg" = "--uninstall" ]; then
            uninstall=1
        fi
    done
    [ "$#" -gt 0 ] || set -- all

    selected=""
    for arg in "$@"; do
        case "$arg" in
            --uninstall) ;;
            all)       selected="$selected acus pons census vicarius" ;;
            acus|pons|census|vicarius) selected="$selected $arg" ;;
            *) err "unknown tool: $arg (use acus, pons, census, vicarius or all)" ;;
        esac
    done

    if [ -n "$uninstall" ]; then
        [ -n "$selected" ] || err "usage: install.sh --uninstall <acus|pons|census|vicarius|all>"
        for tool in $selected; do
            uninstall_tool "$tool"
        done
        return 0
    fi

    os="$(detect_os)"
    target="$(detect_arch)-${os}"
    resolve_token

    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT

    installed=""
    for tool in $selected; do
        install_tool "$tool" "$target"
        installed=1
    done

    if [ -n "$installed" ]; then
        add_to_path
    fi
}

install_tool() {
    tool="$1"
    target="$2"
    asset="${tool}-${target}.tar.gz"

    tag="$(latest_tag "$tool")" || tag=""
    if [ -z "$tag" ]; then
        err "no ${tool} release found in ${REPO}, or GitHub denied the request (rate limit: set GITHUB_TOKEN or run gh auth login)"
    fi

    release_json="$(api_get "${API}/releases/tags/${tag}")" || err "could not read release ${tag} of ${REPO}"
    archive_url="$(printf '%s\n' "$release_json" | asset_api_url "$asset")"
    sum_url="$(printf '%s\n' "$release_json" | asset_api_url "$asset.sha256")"
    if [ -z "$archive_url" ] || [ -z "$sum_url" ]; then
        err "release ${tag} has no asset ${asset}"
    fi

    dir="${tmpdir}/${tool}"
    mkdir -p "$dir"
    say "downloading ${tool} ${tag} for ${target}..."
    api_download "$archive_url" "$dir/$asset" || err "download failed: ${asset}"
    api_download "$sum_url" "$dir/$asset.sha256" || err "download failed: ${asset}.sha256"
    verify_sha256 "$dir/$asset" "$dir/$asset.sha256"

    tar xzf "$dir/$asset" -C "$dir"
    [ -f "$dir/$tool" ] || err "archive ${asset} does not contain ${tool}"
    mkdir -p "$INSTALL_DIR"
    mv "$dir/$tool" "$INSTALL_DIR/$tool"
    chmod +x "$INSTALL_DIR/$tool"
    say "installed ${tool} ${tag} to ${INSTALL_DIR}/${tool}"

    # Installs the agent skill for Claude Code and Codex.
    if [ "$tool" = "acus" ] && [ -z "$NO_SKILL" ]; then
        "$INSTALL_DIR/acus" self skill --install --target all
    fi
}

# Removes the binary from INSTALL_DIR and, for acus, the skill the installer wrote.
# Only a regular file named exactly like the tool is removed; a link or directory aborts.
uninstall_tool() {
    tool="$1"
    path="$INSTALL_DIR/$tool"

    if [ -L "$path" ] || { [ -e "$path" ] && [ ! -f "$path" ]; }; then
        err "refusing to remove ${path}: not a regular file"
    fi
    if [ -f "$path" ]; then
        rm -f "$path"
        say "removed ${path}"
    else
        say "${tool} is not installed in ${INSTALL_DIR}"
    fi

    # The installer writes the skill for Claude Code and Codex; CAPSA_NO_SKILL keeps it.
    if [ "$tool" = "acus" ] && [ -z "$NO_SKILL" ]; then
        remove_skill "$HOME/.claude/skills/acus"
        remove_skill "$HOME/.agents/skills/acus"
    fi
}

# Removes SKILL.md from a skill directory, then the directory when it is empty.
remove_skill() {
    dir="$1"
    file="$dir/SKILL.md"
    if [ -L "$dir" ] || [ -L "$file" ]; then
        err "refusing to remove ${dir}: symbolic link"
    fi
    if [ -f "$file" ]; then
        rm -f "$file"
        say "removed ${file}"
    fi
    if [ -d "$dir" ]; then
        if rmdir "$dir" 2>/dev/null; then
            say "removed ${dir}"
        else
            say "kept ${dir}: it holds other files"
        fi
    fi
}

# Prints the tag of the newest <tool>-v* release. Pages through the release list
# because the newest release may belong to another tool.
latest_tag() {
    page=1
    while [ "$page" -le 10 ]; do
        body="$(api_get "${API}/releases?per_page=100&page=${page}")" || return 1
        case "$body" in
            *'"tag_name"'*) ;;
            *) return 1 ;;
        esac
        tag="$(printf '%s\n' "$body" | split_objects \
            | grep -o "\"tag_name\": *\"$1-v[^\"]*\"" | head -n 1 \
            | sed 's/.*"\([^"]*\)"$/\1/')"
        if [ -n "$tag" ]; then
            printf '%s\n' "$tag"
            return 0
        fi
        page=$((page + 1))
    done
    return 1
}

# Reads release JSON on stdin and prints the API URL of the asset named $1.
# The asset URL (.../releases/assets/ID) comes before "name" in each asset object.
asset_api_url() {
    split_objects | awk -v want="$1" '
        match($0, /"url": *"https:\/\/api\.github\.com\/[^"]+\/releases\/assets\/[0-9]+"/) {
            url = substr($0, RSTART, RLENGTH)
            sub(/^"url": *"/, "", url)
            sub(/"$/, "", url)
        }
        $0 ~ ("\"name\": *\"" want "\"") && url != "" { print url; exit }
    '
}

# One JSON object per line, so the awk lookup sees url and name together.
split_objects() {
    sed 's/}, *{/}\n{/g'
}

verify_sha256() {
    expected="$(awk '{ print tolower($1); exit }' "$2")"
    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$1" | awk '{ print $1 }')"
    else
        actual="$(shasum -a 256 "$1" | awk '{ print $1 }')"
    fi
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
        err "SHA-256 mismatch for $(basename "$1"): expected ${expected:-nothing}, got ${actual}"
    fi
}

# The token goes to curl through its config on stdin, so it stays out of the process list.
curl_github() {
    accept="$1"
    shift
    if [ -n "$TOKEN" ]; then
        printf 'header = "Authorization: Bearer %s"\n' "$TOKEN" \
            | curl -fsSL -K - -H "Accept: ${accept}" "$@"
    else
        curl -fsSL -H "Accept: ${accept}" "$@"
    fi
}

api_get() {
    curl_github "application/vnd.github+json" "$1"
}

# Asset API URLs need Accept: application/octet-stream; curl follows the redirect to storage.
api_download() {
    curl_github "application/octet-stream" -L -o "$2" "$1"
}

resolve_token() {
    if [ -n "${GITHUB_TOKEN:-}" ]; then
        TOKEN="$GITHUB_TOKEN"
    elif command -v gh >/dev/null 2>&1; then
        TOKEN="$(gh auth token 2>/dev/null || true)"
    fi
}

# Adds $INSTALL_DIR to PATH, either for the current user (default, appends to
# the shell rc file) or system-wide (writes /etc/profile.d/capsa.sh, needs
# root). Choose with CAPSA_INSTALL_SCOPE=user|system.
add_to_path() {
    if echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
        return
    fi

    line="export PATH=\"${INSTALL_DIR}:\$PATH\""

    case "$INSTALL_SCOPE" in
        system)
            profile_file="/etc/profile.d/capsa.sh"
            if [ -f "$profile_file" ] && grep -qF "$INSTALL_DIR" "$profile_file"; then
                return
            fi
            if [ -w /etc/profile.d ] || [ "$(id -u)" = "0" ]; then
                printf '%s\n' "$line" > "$profile_file"
                say "added ${INSTALL_DIR} to system-wide PATH via ${profile_file}"
            elif command -v sudo >/dev/null 2>&1; then
                printf '%s\n' "$line" | sudo tee "$profile_file" >/dev/null
                say "added ${INSTALL_DIR} to system-wide PATH via ${profile_file} (sudo)"
            else
                say ""
                say "could not write ${profile_file} (need root), add manually:"
                say "  $line"
            fi
            ;;
        *)
            rc_file="$HOME/.profile"
            case "$(basename "${SHELL:-}")" in
                zsh)  rc_file="$HOME/.zshrc" ;;
                bash) rc_file="$HOME/.bashrc" ;;
            esac
            if [ -f "$rc_file" ] && grep -qF "$INSTALL_DIR" "$rc_file"; then
                return
            fi
            printf '\n# added by capsa installer\n%s\n' "$line" >> "$rc_file"
            say "added ${INSTALL_DIR} to PATH in ${rc_file} (restart your shell or run: source ${rc_file})"
            ;;
    esac
}

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "unknown-linux-gnu" ;;
        Darwin*) echo "apple-darwin" ;;
        *)       err "unsupported OS, use install.ps1 on Windows" ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  echo "x86_64" ;;
        arm64|aarch64) echo "aarch64" ;;
        *)             err "unsupported architecture: $(uname -m)" ;;
    esac
}

say() { printf '%s\n' "$1"; }
err() { say "error: $1" >&2; exit 1; }

main "$@"
