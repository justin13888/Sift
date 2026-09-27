# shellcheck shell=bash
#
# Where the macOS shell's OAuth clients come from — **one per provider kind**. Sourced, never
# run: `mise-tasks/macos` bakes what this resolves into the bundle, and `mise-tasks/release-rc`
# refuses to start unless it resolves a client for every kind the candidate offers. Both read
# it from here so the precondition and the build can never disagree about where a client lives.
# Paths are relative to the repository root, where both tasks run.
#
# **Configuration, not a secret.** A client identifier for a public client appears in every
# authorization URL it generates — that is why PKCE exists — so this is not withheld for
# secrecy. It is out of the repository because it is per-installation: baking one in would make
# every checkout share one client, and a provider's quota, verification status and consent
# screen all attach to the client rather than to the application.
#
# One per kind because a client belongs to one identity platform: an identifier Google issued
# means nothing to Microsoft's. The kinds are the register's opaque strings
# (`crates/providers/sift-registry`), and a kind with no client is absent from the add-account
# choices rather than offered and broken.
#
#   kind   file                                   environment
#   gmail  shells/macos/oauth-client.gmail.txt    SIFT_OAUTH_CLIENT_ID_GMAIL
#          shells/macos/oauth-client.txt          SIFT_OAUTH_CLIENT_ID      (before there were two)
#   graph  shells/macos/oauth-client.graph.txt    SIFT_OAUTH_CLIENT_ID_GRAPH
#
# Within a kind the environment wins over the file, and each row wins over the one below it.

# The first non-empty of: the variable, then each file in order (whitespace stripped).
sift_read_client() {
    local variable="$1"
    shift
    local value="${!variable:-}"
    local file
    for file in "$@"; do
        if [[ -z "$value" && -f "$file" ]]; then
            value="$(tr -d '[:space:]' <"$file")"
        fi
    done
    printf '%s' "$value"
}

# The client identifier for one provider kind, or nothing when that kind has none.
sift_oauth_client() {
    local value
    case "$1" in
        gmail)
            value="$(sift_read_client SIFT_OAUTH_CLIENT_ID_GMAIL shells/macos/oauth-client.gmail.txt)"
            [[ -n "$value" ]] || value="$(sift_read_client SIFT_OAUTH_CLIENT_ID shells/macos/oauth-client.txt)"
            ;;
        graph)
            value="$(sift_read_client SIFT_OAUTH_CLIENT_ID_GRAPH shells/macos/oauth-client.graph.txt)"
            ;;
        *)
            echo "oauth-clients: unknown provider kind '$1'" >&2
            return 1
            ;;
    esac
    printf '%s' "$value"
}

# Where a kind's client is configured, for a message that tells a person how to supply one.
sift_oauth_client_sources() {
    case "$1" in
        gmail) printf '%s' "shells/macos/oauth-client.gmail.txt or SIFT_OAUTH_CLIENT_ID_GMAIL" ;;
        graph) printf '%s' "shells/macos/oauth-client.graph.txt or SIFT_OAUTH_CLIENT_ID_GRAPH" ;;
        *) return 1 ;;
    esac
}
