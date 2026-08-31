//! The authorization profile an adapter *declares*, and the pieces of the flow that are the
//! same for every provider.
//!
//! # Why the endpoints are data rather than code
//!
//! D-12 forbids a provider name above the adapter layer, and the credential broker lives in
//! the application layer. So the broker cannot know Google's token endpoint, Microsoft's
//! scope names, or anybody's consent parameters — and equally, an adapter must not run its
//! own authorization flow, because D-88's rules (single-flight refresh, write-before-use,
//! the failure classifier) would then exist four times and differ four ways.
//!
//! The resolution is the one the capability model already uses: **the adapter declares, the
//! layer above plans.** An [`OAuthProfile`] is a capability row that happens to hold URLs.
//!
//! # What is checked here rather than trusted
//!
//! [`OAuthProfile::authorizes_sending`] is the no-send constraint expressed where a reviewer
//! can check it against an authorization screen, which is what D-88's second point asks for.
//! It is not a lint over the string "send": a scope that grants a mail protocol grants
//! submission over that protocol whatever it is called, and the notable case is a provider
//! whose full-mailbox scope is spelled as a bare URL.

use core::fmt::Write as _;
use sha2::{Digest, Sha256};

/// One host and one path. Split rather than joined so that nothing here has to parse a URL,
/// and so the transport is handed the two things it actually needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub path: String,
}

impl Endpoint {
    #[must_use]
    pub fn new(host: &str, path: &str) -> Self {
        Self {
            host: host.to_owned(),
            path: path.to_owned(),
        }
    }

    #[must_use]
    pub fn url(&self) -> String {
        format!("https://{}{}", self.host, self.path)
    }
}

/// What an adapter declares about how its account is authorized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProfile {
    pub authorize: Endpoint,
    pub token: Endpoint,
    /// Best-effort only. FR-4's erasure is local and provable; revocation is on top, and
    /// removal never blocks on it.
    pub revoke: Option<Endpoint>,
    /// **Permanent.** Widening it later forces the entire install base through re-consent,
    /// which is why D-88 calls the scope set a minimum rather than a starting point.
    pub scopes: Vec<String>,
    /// Parameters the provider requires on the authorization request and nobody else does.
    pub authorize_parameters: Vec<(String, String)>,
    /// Scope strings that grant a mail submission path even though they do not say so.
    ///
    /// Declared by the adapter because only the adapter knows: a provider's full-mailbox
    /// scope is often a bare URL whose name says nothing about what it authorizes.
    pub sending_scopes: Vec<String>,
}

impl OAuthProfile {
    /// Whether any scope requested would authorize sending mail.
    ///
    /// **This must be false forever.** The no-send constraint is structural, and a scope
    /// that authorizes submission is an outbound message path that exists whether or not any
    /// code calls it — it is the capability sitting on the user's consent screen, granted.
    #[must_use]
    pub fn authorizes_sending(&self) -> bool {
        self.scopes.iter().any(|s| {
            let lower = s.to_ascii_lowercase();
            lower.contains("send")
                || lower.contains("compose")
                || lower.contains("submission")
                || self.sending_scopes.iter().any(|k| k == s)
        })
    }
}

/// A proof key, per D-88's first point: a public client with PKCE and **no embedded
/// secret**, because "a secret shipped through three channels is not a secret".
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// Generate a verifier and its S256 challenge.
    ///
    /// # Errors
    /// Where the platform's randomness is unavailable. Refused rather than substituted:
    /// D-71 makes an absent security guarantee a refusal, and a predictable verifier is a
    /// PKCE exchange that proves nothing.
    pub fn generate() -> Result<Self, &'static str> {
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw).map_err(|_| "the platform's randomness was unavailable")?;
        Ok(Self::from_entropy(&raw))
    }

    #[must_use]
    pub fn from_entropy(raw: &[u8]) -> Self {
        let verifier = base64url(raw);
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

/// A state parameter — D-36's, doing real work rather than being ceremony.
///
/// # Errors
/// Where the platform's randomness is unavailable.
pub fn state() -> Result<String, &'static str> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).map_err(|_| "the platform's randomness was unavailable")?;
    Ok(base64url(&raw))
}

/// Unpadded URL-safe base64, which is the encoding every one of these fields uses.
#[must_use]
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let take = chunk.len() + 1;
        for i in 0..take {
            let index = (n >> (18 - 6 * i)) & 0x3f;
            out.push(ALPHABET[index as usize] as char);
        }
    }
    out
}

/// Percent-encode for `application/x-www-form-urlencoded`.
#[must_use]
pub fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// The address the user is sent to.
#[must_use]
pub fn authorization_url(
    profile: &OAuthProfile,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    pkce: &Pkce,
) -> String {
    let mut url = profile.authorize.url();
    url.push('?');
    let mut fields: Vec<(String, String)> = vec![
        ("response_type".into(), "code".into()),
        ("client_id".into(), client_id.into()),
        ("redirect_uri".into(), redirect_uri.into()),
        ("scope".into(), profile.scopes.join(" ")),
        ("state".into(), state.into()),
        ("code_challenge".into(), pkce.challenge.clone()),
        ("code_challenge_method".into(), "S256".into()),
    ];
    fields.extend(profile.authorize_parameters.iter().cloned());
    let query: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
        .collect();
    url.push_str(&query.join("&"));
    url
}

/// The body of the code-for-token exchange.
#[must_use]
pub fn exchange_body(client_id: &str, redirect_uri: &str, code: &str, verifier: &str) -> String {
    form(&[
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("code", code),
        ("code_verifier", verifier),
    ])
}

/// The body of a refresh.
#[must_use]
pub fn refresh_body(client_id: &str, refresh_token: &str) -> String {
    form(&[
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
    ])
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// What the provider granted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenGrant {
    pub access: String,
    /// Absent on a refresh against a provider that does not rotate them. The caller keeps
    /// the one it had rather than treating the absence as a loss.
    pub refresh: Option<String>,
    pub expires_in_secs: Option<u64>,
    /// What was actually granted, which may be narrower than what was asked for.
    pub scope: Option<String>,
}

/// What came back from the token endpoint.
///
/// The three-way split is D-88's classifier, and the reason the third variant exists at all
/// is [`TokenAnswer::Unparseable`]'s doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenAnswer {
    Granted(TokenGrant),
    /// A well-formed provider error explicitly denying the grant — RFC 6749's
    /// `invalid_grant`. **The only non-transient case.**
    GrantDenied {
        error: String,
        description: Option<String>,
    },
    /// A well-formed provider error that is not a denial of the grant. Something about the
    /// request, or about the client's registration — retrying it unchanged will not help,
    /// and re-authenticating the user will not either.
    Refused {
        error: String,
        description: Option<String>,
    },
    /// **A response that does not parse as the provider's own error document.**
    ///
    /// This is the one that matters. A captive portal answering with a sign-in page produces
    /// exactly this, and treating it as authoritative would prompt for re-authentication on
    /// **every account at once** — an application that appears to have lost the user's
    /// credentials, when in fact it is on a hotel network.
    Unparseable,
}

/// Read the token endpoint's answer.
///
/// The status is taken into account but is not trusted on its own: a 200 carrying an error
/// document is an error, and a 400 carrying nothing parseable is not evidence about the
/// grant.
#[must_use]
pub fn read_token_answer(status: u16, body: &[u8]) -> TokenAnswer {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return TokenAnswer::Unparseable;
    };
    if let Some(error) = value.get("error").and_then(|e| e.as_str()) {
        let description = value
            .get("error_description")
            .and_then(|d| d.as_str())
            .map(str::to_owned);
        let error = error.to_owned();
        // RFC 6749 §5.2: `invalid_grant` is the refresh token being expired, revoked, or
        // otherwise no longer valid. Everything else says something about the request.
        return if error == "invalid_grant" {
            TokenAnswer::GrantDenied { error, description }
        } else {
            TokenAnswer::Refused { error, description }
        };
    }
    let Some(access) = value.get("access_token").and_then(|t| t.as_str()) else {
        // A success with no token, or a failure with no error field. Neither is the
        // provider's own error document, so neither is authoritative about the grant.
        let _ = status;
        return TokenAnswer::Unparseable;
    };
    TokenAnswer::Granted(TokenGrant {
        access: access.to_owned(),
        refresh: value
            .get("refresh_token")
            .and_then(|t| t.as_str())
            .map(str::to_owned),
        expires_in_secs: value.get("expires_in").and_then(serde_json::Value::as_u64),
        scope: value
            .get("scope")
            .and_then(|s| s.as_str())
            .map(str::to_owned),
    })
}

/// Whether what was granted covers what was asked for.
///
/// A provider may grant less than was requested. Sift's scope set is already the minimum for
/// read, search and the FR-13 intent set, so a narrower grant is an account that will fail
/// later in a way nothing connects back to the consent screen — better to say so at the
/// moment it happens.
#[must_use]
pub fn granted_covers(requested: &[String], granted: Option<&str>) -> bool {
    let Some(granted) = granted else {
        // The provider did not say. Believed, because the alternative is refusing an account
        // over a field the specification makes optional.
        return true;
    };
    let granted: Vec<&str> = granted.split_whitespace().collect();
    requested
        .iter()
        .all(|want| granted.contains(&want.as_str()))
}

/// Pull one parameter out of a callback address.
///
/// The callback arrives through a registered URI scheme (D-36), so it is a string handed to
/// Sift by the operating system on behalf of **any** process running as the user. Nothing
/// here trusts it beyond reading fields out of it; the state parameter is what decides
/// whether it is answered at all.
#[must_use]
pub fn callback_parameter(callback: &str, name: &str) -> Option<String> {
    let query = callback.split_once('?').map(|(_, q)| q)?;
    query.split('&').find_map(|field| {
        let (k, v) = field.split_once('=')?;
        (k == name).then(|| form_decode(v))
    })
}

#[must_use]
fn form_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = core::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> OAuthProfile {
        OAuthProfile {
            authorize: Endpoint::new("auth.example.test", "/authorize"),
            token: Endpoint::new("token.example.test", "/token"),
            revoke: None,
            scopes: vec!["read".into(), "modify".into()],
            authorize_parameters: vec![("access_type".into(), "offline".into())],
            sending_scopes: vec!["https://mail.example.test/".into()],
        }
    }

    #[test]
    fn the_challenge_is_the_sha256_of_the_verifier_in_url_safe_base64() {
        // RFC 7636's own worked example, so this is checked against the specification
        // rather than against itself.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expect = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(base64url(&Sha256::digest(verifier.as_bytes())), expect);
    }

    #[test]
    fn base64url_matches_the_alphabet_and_drops_the_padding() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
        assert_eq!(base64url(&[0xff, 0xef]), "_-8");
    }

    #[test]
    fn two_verifiers_differ() {
        let a = Pkce::generate().expect("randomness");
        let b = Pkce::generate().expect("randomness");
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.challenge, b.challenge);
        assert!(a.verifier.len() >= 43, "below the specified minimum length");
    }

    #[test]
    fn the_authorization_url_carries_the_challenge_and_never_the_verifier() {
        let pkce = Pkce::from_entropy(&[7u8; 32]);
        let url = authorization_url(&profile(), "client", "net.example:/cb", "st", &pkce);
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", pkce.challenge)));
        assert!(
            !url.contains(&pkce.verifier),
            "the verifier went to the browser"
        );
        assert!(url.contains("scope=read%20modify"));
        assert!(url.contains("access_type=offline"));
        assert!(url.starts_with("https://auth.example.test/authorize?"));
    }

    #[test]
    fn a_redirect_through_a_registered_scheme_survives_encoding() {
        let url = authorization_url(
            &profile(),
            "c",
            "net.justinchung.sift:/oauth2/callback",
            "s",
            &Pkce::from_entropy(&[1u8; 32]),
        );
        assert!(url.contains("redirect_uri=net.justinchung.sift%3A%2Foauth2%2Fcallback"));
    }

    #[test]
    fn no_scope_requested_authorizes_sending() {
        assert!(!profile().authorizes_sending());
    }

    #[test]
    fn a_full_mailbox_scope_is_caught_even_though_its_name_says_nothing() {
        // The case a substring lint misses entirely, and the one that actually arises: a
        // provider's full-access scope is a bare URL that grants submission.
        let mut p = profile();
        p.scopes.push("https://mail.example.test/".into());
        assert!(
            p.authorizes_sending(),
            "a scope granting a mail protocol granted submission unnoticed"
        );
    }

    #[test]
    fn an_obviously_named_send_scope_is_caught_too() {
        for scope in [
            "mail.send",
            "Mail.Compose",
            "urn:ietf:params:jmap:submission",
        ] {
            let mut p = profile();
            p.scopes.push(scope.into());
            assert!(p.authorizes_sending(), "{scope}");
        }
    }

    #[test]
    fn the_exchange_sends_the_verifier_and_no_secret() {
        let body = exchange_body("client", "net.example:/cb", "the-code", "the-verifier");
        assert!(body.contains("code_verifier=the-verifier"));
        assert!(body.contains("grant_type=authorization_code"));
        assert!(
            !body.contains("client_secret"),
            "a public client sent a secret"
        );
    }

    #[test]
    fn a_refresh_sends_no_secret_either() {
        let body = refresh_body("client", "r");
        assert!(!body.contains("client_secret"));
        assert!(body.contains("grant_type=refresh_token"));
    }

    #[test]
    fn a_grant_is_read_with_everything_the_caller_needs() {
        let answer = read_token_answer(
            200,
            br#"{"access_token":"at","refresh_token":"rt","expires_in":3599,"scope":"a b"}"#,
        );
        assert_eq!(
            answer,
            TokenAnswer::Granted(TokenGrant {
                access: "at".into(),
                refresh: Some("rt".into()),
                expires_in_secs: Some(3599),
                scope: Some("a b".into()),
            })
        );
    }

    #[test]
    fn a_refresh_that_returns_no_new_refresh_token_is_not_a_loss() {
        let TokenAnswer::Granted(grant) = read_token_answer(200, br#"{"access_token":"at"}"#)
        else {
            panic!("not granted");
        };
        assert_eq!(grant.refresh, None);
    }

    #[test]
    fn only_an_explicit_denial_of_the_grant_is_a_denial() {
        assert!(matches!(
            read_token_answer(
                400,
                br#"{"error":"invalid_grant","error_description":"expired"}"#
            ),
            TokenAnswer::GrantDenied { .. }
        ));
        // Something about the request or the registration. Re-authenticating the user does
        // not help, so this must not present as "please sign in again".
        assert!(matches!(
            read_token_answer(401, br#"{"error":"invalid_client"}"#),
            TokenAnswer::Refused { .. }
        ));
    }

    #[test]
    fn a_captive_portals_sign_in_page_is_unparseable_rather_than_a_denial() {
        // The specific failure D-88's classifier exists to prevent: treating this as
        // authoritative prompts for re-authentication on every account at once.
        for body in [
            &b"<html>Sign in to the network</html>"[..],
            b"",
            b"Service Unavailable",
            b"{}",
            b"{\"error_description\":\"no error field\"}",
        ] {
            assert_eq!(
                read_token_answer(400, body),
                TokenAnswer::Unparseable,
                "{body:?}"
            );
        }
    }

    #[test]
    fn a_success_carrying_an_error_document_is_an_error() {
        assert!(matches!(
            read_token_answer(200, br#"{"error":"invalid_grant"}"#),
            TokenAnswer::GrantDenied { .. }
        ));
    }

    #[test]
    fn a_narrower_grant_than_was_asked_for_is_noticed() {
        let want = vec!["read".to_owned(), "modify".to_owned()];
        assert!(granted_covers(&want, Some("read modify extra")));
        assert!(!granted_covers(&want, Some("read")));
        // A provider that does not say is believed: the field is optional, and refusing an
        // account over its absence would be worse than the failure it prevents.
        assert!(granted_covers(&want, None));
    }

    #[test]
    fn a_callback_is_read_field_by_field() {
        let cb = "net.justinchung.sift:/oauth2/callback?state=abc&code=4%2F0Ab%2Bc";
        assert_eq!(callback_parameter(cb, "state").as_deref(), Some("abc"));
        assert_eq!(callback_parameter(cb, "code").as_deref(), Some("4/0Ab+c"));
        assert_eq!(callback_parameter(cb, "absent"), None);
    }

    #[test]
    fn a_callback_carrying_a_denial_is_read_rather_than_mistaken_for_a_code() {
        let cb = "net.justinchung.sift:/oauth2/callback?error=access_denied&state=abc";
        assert_eq!(callback_parameter(cb, "code"), None);
        assert_eq!(
            callback_parameter(cb, "error").as_deref(),
            Some("access_denied")
        );
    }

    #[test]
    fn a_callback_with_no_query_at_all_yields_nothing_rather_than_panicking() {
        // Any process running as the user can invoke the scheme with anything.
        assert_eq!(callback_parameter("net.justinchung.sift:", "state"), None);
        assert_eq!(callback_parameter("", "state"), None);
        assert_eq!(callback_parameter("?", "state"), None);
        assert_eq!(callback_parameter("?%", "state"), None);
        assert_eq!(
            callback_parameter("?state=%zz", "state").as_deref(),
            Some("%zz")
        );
    }
}
