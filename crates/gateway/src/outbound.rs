//! The one hardened outbound HTTP client (issue #9 — SSRF guard).
//!
//! Every server-side fetch of a URL that a user or a federated peer supplied goes
//! through here. Without it the platform is a request proxy into whatever network
//! the pod can reach: the inbox takes an actor URL straight from an UNAUTHENTICATED
//! `Signature` header's `keyId`, webhooks take a URL from an admin form, and the
//! intercoms SMS provider URL carries citizens' phone OTPs.
//!
//! A `starts_with("https://")` check is not a defence. `https://10.0.0.1/` passes it,
//! and so does a public host that redirects to one, or resolves to one.
//!
//! Four properties, in the order they matter:
//!
//! 1. **Scheme allowlist.** HTTPS only, unless an operator opts a surface into plain
//!    HTTP explicitly.
//! 2. **Address validation with PINNED resolution.** The host is resolved once, every
//!    returned address is checked against the non-routable ranges, and the connection
//!    is then pinned to those exact addresses. Validating and re-resolving would leave
//!    a DNS-rebinding window between the two; pinning closes it, because the address
//!    that was checked is the address that gets dialled.
//! 3. **No redirects.** A 302 to `http://169.254.169.254/` would otherwise walk
//!    straight past checks 1 and 2. Following redirects safely means revalidating
//!    every hop; not following them at all is simpler and enough for these surfaces.
//! 4. **Bounded body.** The reply is read in chunks and abandoned past the cap, so a
//!    hostile peer cannot exhaust memory with an endless response.
//!
//! IPv6 is first-class here (PLAN.md): production is IPv6-only, so the guard must
//! block the IPv6 non-routable ranges as carefully as the IPv4 ones — and must NOT
//! block ordinary global unicast, or it would break federation entirely.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use futures_util::StreamExt as _;
use reqwest::Url;

/// Default cap on a response body. Actor documents and webhook replies are small;
/// anything larger is either a mistake or an attempt to exhaust us.
pub const DEFAULT_MAX_BODY: usize = 1024 * 1024;

/// Default timeout for a guarded request.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Why an outbound request was refused before or during the fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundError {
    /// The URL could not be parsed.
    Malformed,
    /// The scheme is not allowed for this surface (HTTPS is required by default).
    SchemeNotAllowed(String),
    /// The URL carries no host.
    NoHost,
    /// DNS resolution failed or returned nothing.
    Unresolvable,
    /// The host resolves to (or literally is) an address we refuse to dial.
    BlockedAddress(IpAddr),
    /// The destination is not in the operator's allowlist for this surface.
    NotAllowlisted(String),
    /// The response body exceeded the cap.
    BodyTooLarge(usize),
    /// The request failed for a transport reason (timeout, TLS, refused, redirect).
    Transport(String),
    /// The peer answered with a non-success status.
    Status(u16),
}

impl std::fmt::Display for OutboundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(f, "malformed URL"),
            Self::SchemeNotAllowed(s) => write!(f, "scheme not allowed: {s}"),
            Self::NoHost => write!(f, "URL has no host"),
            Self::Unresolvable => write!(f, "host does not resolve"),
            Self::BlockedAddress(ip) => write!(f, "refusing to dial a non-routable address: {ip}"),
            Self::NotAllowlisted(h) => write!(f, "destination not allowlisted: {h}"),
            Self::BodyTooLarge(cap) => write!(f, "response body exceeded {cap} bytes"),
            Self::Transport(e) => write!(f, "transport error: {e}"),
            Self::Status(s) => write!(f, "remote returned {s}"),
        }
    }
}

impl std::error::Error for OutboundError {}

/// Per-surface policy. Federation uses the default; webhooks and intercoms may be
/// widened by an operator who knowingly accepts the risk.
#[derive(Debug, Clone)]
pub struct OutboundPolicy {
    /// Permit plain `http://`. Off by default; only an operator should turn it on.
    pub allow_http: bool,
    /// When set, the host must match one of these entries (exact, case-insensitive,
    /// or a leading `.` for a suffix match). `None` = any routable host.
    pub allowlist: Option<Vec<String>>,
    /// Maximum bytes accepted from the response body.
    pub max_body: usize,
    /// Whole-request timeout.
    pub timeout: Duration,
}

impl Default for OutboundPolicy {
    fn default() -> Self {
        Self {
            allow_http: false,
            allowlist: None,
            max_body: DEFAULT_MAX_BODY,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl OutboundPolicy {
    /// Read an operator allowlist from `var` (comma-separated hosts). Absent or
    /// empty leaves the policy unrestricted, which is the current behaviour of
    /// every surface — this exists so an operator CAN lock a surface down, not to
    /// silently change what happens today.
    #[must_use]
    pub fn with_allowlist_from_env(mut self, var: &str) -> Self {
        let raw = std::env::var(var).unwrap_or_default();
        let hosts: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if !hosts.is_empty() {
            self.allowlist = Some(hosts);
        }
        self
    }
}

/// Is this address one we refuse to dial?
///
/// Deliberately a denylist of NON-ROUTABLE space rather than an allowlist of public
/// space: the set of "addresses that must never be reachable from a URL a stranger
/// chose" is the one that is small, well-defined and stable.
#[must_use]
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => {
            // An IPv4-mapped address (::ffff:10.0.0.1) is an IPv4 destination
            // wearing an IPv6 costume — judge it as what it dials.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_v4(mapped);
            }
            is_blocked_v6(v6)
        }
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()          // 127.0.0.0/8
        || ip.is_private()    // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local() // 169.254/16 — includes the cloud metadata endpoint
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()      // 0.0.0.0
        || ip.is_documentation()
        || a == 0                   // 0.0.0.0/8 "this network"
        || (a == 100 && (64..128).contains(&b)) // 100.64/10 carrier-grade NAT
        || (a == 192 && b == 0)     // 192.0.0.0/24 IETF protocol assignments
        || a >= 240 // 240/4 reserved, and 255.255.255.255
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    ip.is_loopback()        // ::1
        || ip.is_unspecified() // ::
        || ip.is_multicast()
        || (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
        || (seg[0] & 0xffc0) == 0xfe80 // fe80::/10 link local
        || seg[0] == 0x2001 && seg[1] == 0x0db8 // 2001:db8::/32 documentation
        || ip.to_ipv4().is_some_and(is_blocked_v4) // ::a.b.c.d compatible form
}

/// Does `host` satisfy `allowlist`? An entry beginning with `.` matches any subdomain.
#[must_use]
pub fn host_allowed(host: &str, allowlist: Option<&[String]>) -> bool {
    let Some(list) = allowlist else {
        return true;
    };
    let host = host.to_ascii_lowercase();
    list.iter().any(|entry| {
        if let Some(suffix) = entry.strip_prefix('.') {
            host == suffix || host.ends_with(&format!(".{suffix}"))
        } else {
            host == *entry
        }
    })
}

/// Parse and check everything about a URL that does not need the network.
///
/// # Errors
/// Returns the specific [`OutboundError`] the URL violates.
pub fn validate_url(raw: &str, policy: &OutboundPolicy) -> Result<Url, OutboundError> {
    let url = Url::parse(raw).map_err(|_| OutboundError::Malformed)?;
    let scheme = url.scheme().to_ascii_lowercase();
    let scheme_ok = scheme == "https" || (policy.allow_http && scheme == "http");
    if !scheme_ok {
        return Err(OutboundError::SchemeNotAllowed(scheme));
    }
    // An empty host is rejected explicitly rather than left to fail later at DNS:
    // it must never reach the allowlist or the literal-address check as `""`.
    let host = url.host_str().unwrap_or_default().to_owned();
    if host.is_empty() {
        return Err(OutboundError::NoHost);
    }
    if !host_allowed(&host, policy.allowlist.as_deref()) {
        return Err(OutboundError::NotAllowlisted(host));
    }
    // A literal address in the URL is checked here, before any DNS work happens.
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(OutboundError::BlockedAddress(ip));
        }
    }
    Ok(url)
}

/// Resolve `url`'s host and return the addresses, having rejected the request if ANY
/// of them is non-routable.
///
/// All-or-nothing on purpose: a name that resolves to both a public and a private
/// address is exactly the DNS-rebinding shape, and picking the "good" one would be
/// racing the attacker's TTL.
async fn resolve_checked(url: &Url) -> Result<Vec<SocketAddr>, OutboundError> {
    let host = url.host_str().ok_or(OutboundError::NoHost)?;
    let port = url
        .port_or_known_default()
        .ok_or(OutboundError::Malformed)?;
    let host_for_dns = host.trim_matches(['[', ']']).to_owned();
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host_for_dns, port))
        .await
        .map_err(|_| OutboundError::Unresolvable)?
        .collect();
    if addrs.is_empty() {
        return Err(OutboundError::Unresolvable);
    }
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            return Err(OutboundError::BlockedAddress(addr.ip()));
        }
    }
    Ok(addrs)
}

/// Build a client pinned to `addrs` for `host`, refusing redirects.
fn pinned_client(
    host: &str,
    addrs: &[SocketAddr],
    policy: &OutboundPolicy,
) -> Result<reqwest::Client, OutboundError> {
    let mut builder = reqwest::Client::builder()
        .timeout(policy.timeout)
        // No redirects: a 302 into internal space would bypass every check above.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(crate::outbound::USER_AGENT);
    // `resolve_to_addrs` is the pinning: reqwest dials THESE addresses for this host
    // and performs no second lookup, so the address we validated is the one used.
    builder = builder.resolve_to_addrs(host, addrs);
    builder
        .build()
        .map_err(|e| OutboundError::Transport(e.to_string()))
}

/// Identifies this instance honestly to the peers it fetches from.
pub const USER_AGENT: &str = concat!("agora-core/", env!("CARGO_PKG_VERSION"));

/// Read a response body, refusing anything past `max_body`.
///
/// The declared `Content-Length` is checked first so an oversized reply costs nothing,
/// but the streamed accounting is what actually enforces the cap — a hostile peer can
/// simply omit or lie about the header.
async fn read_capped(resp: reqwest::Response, max_body: usize) -> Result<Vec<u8>, OutboundError> {
    let declared = resp.content_length();
    read_capped_stream(resp.bytes_stream(), declared, max_body).await
}

/// The cap itself, over any chunk stream.
///
/// Split out from [`read_capped`] so it can be tested against synthetic chunks: an
/// integration test cannot reach this code, because every address a local test server
/// can bind is one the address guard refuses first. A test that "passed" through the
/// address check while claiming to cover the cap would be measuring nothing.
async fn read_capped_stream<S, E>(
    stream: S,
    declared_len: Option<u64>,
    max_body: usize,
) -> Result<Vec<u8>, OutboundError>
where
    S: futures_util::Stream<Item = Result<bytes::Bytes, E>>,
    E: std::fmt::Display,
{
    // A declared length over the cap costs us nothing to refuse up front...
    if let Some(len) = declared_len {
        if usize::try_from(len).is_ok_and(|l| l > max_body) {
            return Err(OutboundError::BodyTooLarge(max_body));
        }
    }
    // ...but the streamed accounting is what ENFORCES it, because a hostile peer can
    // omit Content-Length or simply lie in it.
    let mut out: Vec<u8> = Vec::new();
    let stream = std::pin::pin!(stream);
    let mut stream = stream;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| OutboundError::Transport(e.to_string()))?;
        if out.len() + chunk.len() > max_body {
            return Err(OutboundError::BodyTooLarge(max_body));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// GET `raw_url` through the guard, returning the body bytes.
///
/// `headers` are applied verbatim — federation needs `Accept` plus the signed-fetch
/// `Host`/`Date`/`Signature` triple.
///
/// # Errors
/// Returns the [`OutboundError`] describing which guard refused, or the transport
/// failure. Every rejection is logged at warn with the URL, because a burst of them
/// is what an SSRF probe looks like from the operator's side.
pub async fn guarded_get(
    raw_url: &str,
    headers: &[(String, String)],
    policy: &OutboundPolicy,
) -> Result<Vec<u8>, OutboundError> {
    let url = validate_url(raw_url, policy).inspect_err(|err| {
        tracing::warn!(url = raw_url, error = %err, "outbound GET refused by the SSRF guard");
    })?;
    let addrs = resolve_checked(&url).await.inspect_err(|err| {
        tracing::warn!(url = raw_url, error = %err, "outbound GET refused by the SSRF guard");
    })?;
    let host = url.host_str().unwrap_or_default().to_owned();
    let client = pinned_client(&host, &addrs, policy)?;

    let mut req = client.get(url.clone());
    for (name, value) in headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| OutboundError::Transport(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(OutboundError::Status(status.as_u16()));
    }
    read_capped(resp, policy.max_body).await
}

/// POST `body` to `raw_url` through the guard, returning `(status, body)`.
///
/// Used by the surfaces that SEND (webhooks, intercoms, federation delivery), where
/// the caller needs the status even when it is not a success.
///
/// # Errors
/// Returns the [`OutboundError`] describing which guard refused, or the transport failure.
pub async fn guarded_post(
    raw_url: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
    policy: &OutboundPolicy,
) -> Result<(u16, Vec<u8>), OutboundError> {
    let url = validate_url(raw_url, policy).inspect_err(|err| {
        tracing::warn!(url = raw_url, error = %err, "outbound POST refused by the SSRF guard");
    })?;
    let addrs = resolve_checked(&url).await.inspect_err(|err| {
        tracing::warn!(url = raw_url, error = %err, "outbound POST refused by the SSRF guard");
    })?;
    let host = url.host_str().unwrap_or_default().to_owned();
    let client = pinned_client(&host, &addrs, policy)?;

    let mut req = client.post(url.clone()).body(body);
    for (name, value) in headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| OutboundError::Transport(e.to_string()))?;
    let status = resp.status().as_u16();
    let bytes = read_capped(resp, policy.max_body).await?;
    Ok((status, bytes))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_the_ipv4_ranges_an_ssrf_aims_at() {
        for s in [
            "127.0.0.1",    // loopback
            "127.10.20.30", // the rest of 127/8
            "10.0.0.1",     // RFC1918
            "172.16.5.4",
            "192.168.1.1",
            "169.254.169.254", // cloud metadata — the classic target
            "0.0.0.0",
            "0.1.2.3",
            "100.64.0.1", // carrier-grade NAT
            "192.0.0.1",
            "224.0.0.1", // multicast
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(is_blocked_ip(ip(s)), "{s} must be blocked");
        }
    }

    #[test]
    fn blocks_the_ipv6_ranges_too() {
        for s in [
            "::1",             // loopback
            "::",              // unspecified
            "fc00::1",         // unique local
            "fd12:3456::1",    // unique local
            "fe80::1",         // link local
            "ff02::1",         // multicast
            "2001:db8::1",     // documentation
            "::ffff:10.0.0.1", // IPv4-mapped private — the costume
            "::ffff:127.0.0.1",
        ] {
            assert!(is_blocked_ip(ip(s)), "{s} must be blocked");
        }
    }

    #[test]
    fn allows_ordinary_public_addresses() {
        // Blocking too much would break federation. This instance's own production
        // prefix is IPv6 global unicast, so that case is pinned explicitly.
        for s in [
            "1.1.1.1",
            "93.184.216.34",
            "2804:710:d0:9::a000", // the sovereign VM
            "2606:4700:4700::1111",
        ] {
            assert!(!is_blocked_ip(ip(s)), "{s} must be allowed");
        }
    }

    #[test]
    fn requires_https_by_default() {
        let p = OutboundPolicy::default();
        assert_eq!(
            validate_url("http://example.com/x", &p).unwrap_err(),
            OutboundError::SchemeNotAllowed("http".to_owned())
        );
        for raw in ["file:///etc/passwd", "gopher://x/", "ftp://x/"] {
            assert!(matches!(
                validate_url(raw, &p),
                Err(OutboundError::SchemeNotAllowed(_) | OutboundError::Malformed)
            ));
        }
        assert!(validate_url("https://example.com/x", &p).is_ok());
    }

    #[test]
    fn http_only_when_an_operator_opts_in() {
        let p = OutboundPolicy {
            allow_http: true,
            ..Default::default()
        };
        assert!(validate_url("http://example.com/x", &p).is_ok());
    }

    #[test]
    fn rejects_a_literal_internal_address_without_touching_dns() {
        let p = OutboundPolicy::default();
        // `starts_with("https://")` — the check this replaces — accepts every one.
        for raw in [
            "https://127.0.0.1/actor",
            "https://10.0.0.5/actor",
            "https://169.254.169.254/latest/meta-data/",
            "https://[::1]/actor",
            "https://[fd00::1]/actor",
        ] {
            assert!(
                matches!(validate_url(raw, &p), Err(OutboundError::BlockedAddress(_))),
                "{raw} must be refused"
            );
        }
    }

    #[test]
    fn allowlist_matches_exactly_or_by_suffix() {
        let list = vec!["sms.example.com".to_owned(), ".trusted.org".to_owned()];
        assert!(host_allowed("sms.example.com", Some(&list)));
        assert!(host_allowed("SMS.EXAMPLE.COM", Some(&list)));
        assert!(host_allowed("trusted.org", Some(&list)));
        assert!(host_allowed("a.b.trusted.org", Some(&list)));
        assert!(!host_allowed("evil.com", Some(&list)));
        // A suffix entry must not match a lookalike registered domain.
        assert!(!host_allowed("nottrusted.org", Some(&list)));
        assert!(!host_allowed("sms.example.com.evil.com", Some(&list)));
        // No list = unrestricted.
        assert!(host_allowed("anything.example", None));
    }

    #[test]
    fn allowlist_refusal_is_reported_as_such() {
        let p = OutboundPolicy {
            allowlist: Some(vec!["good.example".to_owned()]),
            ..Default::default()
        };
        assert_eq!(
            validate_url("https://bad.example/x", &p).unwrap_err(),
            OutboundError::NotAllowlisted("bad.example".to_owned())
        );
        assert!(validate_url("https://good.example/x", &p).is_ok());
    }

    #[test]
    fn malformed_urls_are_refused_without_panicking() {
        let p = OutboundPolicy::default();
        for raw in ["", "not a url", "https://", "://x", "https://[::zz]/"] {
            assert!(validate_url(raw, &p).is_err(), "{raw:?} must be refused");
        }
    }

    #[test]
    fn a_single_label_authority_is_a_host_not_a_path() {
        // `https:///path` does NOT parse to an empty host — the parser reads `path`
        // as a single-label HOSTNAME. It is therefore a well-formed URL, the URL
        // check passes, and the refusal correctly lands at resolution instead.
        // Pinned because "that has no host" is the intuitive and wrong reading, and
        // acting on it would have meant a guard that rejects for the wrong reason.
        let p = OutboundPolicy::default();
        let url = validate_url("https:///path", &p).expect("valid: host is `path`");
        assert_eq!(url.host_str(), Some("path"));
    }

    #[test]
    fn errors_render_distinctly_for_the_operator() {
        let rendered: Vec<String> = vec![
            OutboundError::Malformed.to_string(),
            OutboundError::SchemeNotAllowed("http".into()).to_string(),
            OutboundError::NoHost.to_string(),
            OutboundError::Unresolvable.to_string(),
            OutboundError::BlockedAddress(ip("10.0.0.1")).to_string(),
            OutboundError::NotAllowlisted("x".into()).to_string(),
            OutboundError::BodyTooLarge(10).to_string(),
            OutboundError::Status(503).to_string(),
        ];
        let unique: std::collections::HashSet<&String> = rendered.iter().collect();
        assert_eq!(unique.len(), rendered.len());
    }

    fn chunks(sizes: &[usize]) -> impl futures_util::Stream<Item = Result<bytes::Bytes, String>> {
        let items: Vec<Result<bytes::Bytes, String>> = sizes
            .iter()
            .map(|n| Ok(bytes::Bytes::from(vec![b'x'; *n])))
            .collect();
        futures_util::stream::iter(items)
    }

    #[tokio::test]
    async fn body_within_the_cap_is_returned_whole() {
        let out = read_capped_stream(chunks(&[100, 100, 55]), Some(255), 1024)
            .await
            .unwrap();
        assert_eq!(out.len(), 255);
    }

    #[tokio::test]
    async fn an_oversized_declared_length_is_refused_before_reading() {
        let err = read_capped_stream(chunks(&[10]), Some(999_999), 1024)
            .await
            .unwrap_err();
        assert_eq!(err, OutboundError::BodyTooLarge(1024));
    }

    #[tokio::test]
    async fn a_lying_content_length_does_not_defeat_the_cap() {
        // The peer declares 10 bytes and then streams 4 KiB. The declared-length
        // short-circuit is an optimisation; the streamed accounting is the control.
        let err = read_capped_stream(chunks(&[1024, 1024, 1024, 1024]), Some(10), 1024)
            .await
            .unwrap_err();
        assert_eq!(err, OutboundError::BodyTooLarge(1024));
    }

    #[tokio::test]
    async fn an_absent_content_length_does_not_defeat_the_cap() {
        // Chunked transfer encoding declares no length at all.
        let err = read_capped_stream(chunks(&[600, 600]), None, 1024)
            .await
            .unwrap_err();
        assert_eq!(err, OutboundError::BodyTooLarge(1024));
    }

    #[tokio::test]
    async fn the_cap_is_a_boundary_not_an_approximation() {
        assert!(read_capped_stream(chunks(&[1024]), None, 1024)
            .await
            .is_ok());
        assert!(read_capped_stream(chunks(&[1025]), None, 1024)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn a_loopback_host_is_refused_at_resolution() {
        // `localhost` is a public-looking NAME that resolves into blocked space —
        // the case a literal-address check alone would miss.
        let p = OutboundPolicy::default();
        let err = guarded_get("https://localhost/actor", &[], &p)
            .await
            .unwrap_err();
        assert!(
            matches!(err, OutboundError::BlockedAddress(_)),
            "expected a blocked address, got {err:?}"
        );
    }
}
