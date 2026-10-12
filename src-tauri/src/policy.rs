use crate::traits::PolicyEnforcer;

pub struct PolicyManager {
    allowed_domains: Vec<String>,
}

impl Default for PolicyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyManager {
    pub fn new() -> Self {
        Self {
            allowed_domains: vec![
                "notion.so".to_string(),
                "www.notion.so".to_string(),
                "notion.com".to_string(),
                "www.notion.com".to_string(),
                "msgstore.www.notion.so".to_string(),
                "file.notion.so".to_string(),
                // Publicly shared pages ("Share to web") live on *.notion.site.
                "notion.site".to_string(),
            ],
        }
    }

    fn is_official_notion(&self, host: &str) -> bool {
        self.allowed_domains
            .iter()
            .any(|domain| host == domain || host.ends_with(&format!(".{}", domain)))
    }

    fn is_tracker(host: &str) -> bool {
        const TRACKERS: [&str; 5] = [
            "googletagmanager.com",
            "google-analytics.com",
            "amplitude.com",
            "mixpanel.com",
            "segment.com",
        ];
        TRACKERS
            .iter()
            .any(|t| host == *t || host.ends_with(&format!(".{}", t)))
    }

    fn is_trusted_oauth(&self, host: &str) -> bool {
        // Sign-in and import providers Notion desktop keeps in-app (SSO
        // redirects must stay in the webview to hand the session back).
        let trusted = [
            "accounts.google.com",
            "appleid.apple.com",
            "login.microsoftonline.com",
            "login.live.com",
            "autologon.microsoftazuread-sso.com",
            "okta.com",
            "onelogin.com",
            "jumpcloud.com",
            "duosecurity.com",
            "atlassian.com",
            "app.asana.com",
            "trello.com",
            "www.evernote.com",
        ];
        trusted
            .iter()
            .any(|t| host == *t || host.ends_with(&format!(".{}", t)))
    }
}

impl PolicyEnforcer for PolicyManager {
    fn validate_url(&self, url: &str) -> bool {
        log::debug!("PolicyManager: Validating URL: {}", url);
        // Parse URL and check if host is in allowed_domains
        if let Ok(parsed_url) = url::Url::parse(url) {
            if parsed_url.scheme() != "https" {
                log::warn!("PolicyManager: BLOCKED non-HTTPS URL scheme: {}", url);
                return false;
            }

            if let Some(host) = parsed_url.host_str() {
                if self.is_official_notion(host) {
                    log::debug!("PolicyManager: ALLOWED official Notion domain: {}", host);
                    return true;
                }

                if self.is_trusted_oauth(host) {
                    log::debug!("PolicyManager: ALLOWED OAuth provider: {}", host);
                    return true;
                }

                log::warn!("PolicyManager: BLOCKED unauthorized endpoint: {}", host);
                return false;
            }
        }

        log::warn!(
            "PolicyManager: BLOCKED malformed or unsupported URL: {}",
            url
        );
        // Block other protocols/malformed URLs by default (Zero-Trust)
        false
    }

    fn telemetry_allowed(&self) -> bool {
        // Manifesto Part III: Anti-Telemetry by Default
        false
    }

    fn validate_external_link(&self, url: &str) -> bool {
        // For external links, we apply a broader security check.
        // For now, allow https only and block common malicious protocols.
        if let Ok(parsed_url) = url::Url::parse(url) {
            // Block known tracker and telemetry domains from jumping out to the browser
            if let Some(host) = parsed_url.host_str() {
                if Self::is_tracker(host) {
                    log::warn!("PolicyManager: BLOCKED tracker/telemetry domain: {}", host);
                    return false;
                }
            }

            match parsed_url.scheme() {
                "https" => true,
                "mailto" => true,
                _ => {
                    log::warn!(
                        "PolicyManager: BLOCKED unsafe external link protocol: {}",
                        parsed_url.scheme()
                    );
                    false
                }
            }
        } else {
            false
        }
    }

    fn should_route_popup_to_system_browser(&self, url: &str) -> bool {
        if let Ok(parsed_url) = url::Url::parse(url) {
            if let Some(host) = parsed_url.host_str() {
                // If it's an official Notion domain or OAuth provider, keep it in the app
                if self.is_official_notion(host) || self.is_trusted_oauth(host) {
                    return false;
                }
            }

            // Local tauri schemes stay in-app
            if parsed_url.scheme() == "tauri" {
                return false;
            }
        }

        // Everything else is an external link and should open in system browser
        true
    }

    fn validate_frame_url(&self, url: &str) -> bool {
        let Ok(parsed_url) = url::Url::parse(url) else {
            return false;
        };
        if parsed_url.scheme() == "https" && self.validate_url(url) {
            return true;
        }
        match parsed_url.scheme() {
            // Local, content-less documents Notion uses for printing, clipboard
            // handling and embed placeholders. They cannot load remote content.
            "about" => matches!(parsed_url.path(), "blank" | "srcdoc"),
            // Blob URLs are origin-bound object URLs (file previews/exports).
            "blob" => true,
            "https" => match parsed_url.host_str() {
                Some(host) if Self::is_tracker(host) => {
                    log::warn!("PolicyManager: BLOCKED tracker frame: {}", host);
                    false
                }
                Some(_) => true,
                None => false,
            },
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::PolicyEnforcer;

    #[test]
    fn test_should_route_popup_to_system_browser() {
        let policy = PolicyManager::new();
        // OAuth providers should NOT be routed (they stay in app to capture session)
        assert!(!policy.should_route_popup_to_system_browser("https://accounts.google.com/o/oauth2/v2/auth"));
        assert!(!policy.should_route_popup_to_system_browser("https://appleid.apple.com/auth"));

        // External links (e.g., GitHub, Slack) should be routed to system browser
        assert!(policy.should_route_popup_to_system_browser("https://github.com/login"));
        assert!(policy.should_route_popup_to_system_browser("https://slack.com/signin"));

        // Notion internal popups should NOT be routed
        assert!(!policy.should_route_popup_to_system_browser("https://www.notion.so/some-popup"));
        assert!(!policy.should_route_popup_to_system_browser("https://www.notion.com/some-popup"));

        // Local tauri schemes should NOT be routed
        assert!(!policy.should_route_popup_to_system_browser("tauri://localhost/index.html"));
    }

    #[test]
    fn test_validate_official_notion_urls() {
        let policy = PolicyManager::new();
        assert!(policy.validate_url("https://www.notion.so"));
        assert!(policy.validate_url("https://www.notion.so/login"));
        assert!(policy.validate_url("https://notion.com/some-page"));
        assert!(policy.validate_url("https://msgstore.www.notion.so/v1/health"));
        assert!(policy.validate_url("https://acme.notion.site/Roadmap-123"));
        assert!(!policy.validate_url("https://notion.site.evil.com"));
        
        // OAuth providers allowed during login
        assert!(policy.validate_url("https://accounts.google.com/auth"));
        assert!(policy.validate_url("https://appleid.apple.com/auth"));
        // Enterprise SSO providers stay in-app, like Notion desktop
        assert!(policy.validate_url("https://login.microsoftonline.com/common/oauth2"));
        assert!(policy.validate_url("https://acme.okta.com/app/sso"));
        assert!(!policy.validate_url("https://okta.com.evil.io/"));
    }

    #[test]
    fn test_block_unauthorized_urls() {
        let policy = PolicyManager::new();
        assert!(!policy.validate_url("https://google.com"));
        assert!(!policy.validate_url("https://facebook.com"));
        assert!(!policy.validate_url("https://malicious-site.com"));

        // Zero-Trust: Subdomain/Suffix Attacks
        assert!(!policy.validate_url("https://evilnotion.so"));
        assert!(!policy.validate_url("https://notion.so.evil.com"));
        assert!(!policy.validate_url("https://accounts.google.com.evil.com"));
        assert!(!policy.validate_url("https://malicious-site.com/notion.so"));

        // Protocol security
        assert!(!policy.validate_url("http://www.notion.so")); // Enforce HTTPS
        assert!(!policy.validate_url("javascript:alert(1)"));
        assert!(!policy.validate_url("data:text/html,base64..."));
    }

    #[test]
    fn test_telemetry_always_blocked() {
        let policy = PolicyManager::new();
        assert!(!policy.telemetry_allowed());
    }

    #[test]
    fn test_validate_external_links() {
        let policy = PolicyManager::new();
        // Repository and support links should work (they open in browser)
        assert!(policy.validate_external_link("https://github.com/diegoakanotoperator/lotion-rs"));
        assert!(policy.validate_external_link("https://github.com/puneetsl/lotion"));
        assert!(policy.validate_external_link("mailto:support@notion.so"));

        // Block unsafe protocols for external browser redirection
        assert!(!policy.validate_external_link("http://unsecure-link.com"));
        assert!(!policy.validate_external_link("javascript:alert('XSS')"));
        assert!(!policy.validate_external_link("file:///etc/passwd"));
        
        // Block trackers/analytics even for external clicks
        assert!(!policy.validate_external_link("https://www.googletagmanager.com/gtm.js"));
        assert!(!policy.validate_external_link("https://www.google-analytics.com/collect"));
    }

    #[test]
    fn test_validate_frame_urls() {
        let policy = PolicyManager::new();
        // Notion's own documents and internal blank frames
        assert!(policy.validate_frame_url("https://www.notion.so/page"));
        assert!(policy.validate_frame_url("about:blank"));
        assert!(policy.validate_frame_url("about:srcdoc"));
        assert!(policy.validate_frame_url("blob:https://www.notion.so/2f1c"));
        // Third-party embeds
        assert!(policy.validate_frame_url("https://www.youtube.com/embed/abc"));
        assert!(policy.validate_frame_url("https://www.figma.com/embed?url=x"));
        // Trackers, insecure and dangerous schemes stay blocked
        assert!(!policy.validate_frame_url("https://www.googletagmanager.com/ns.html"));
        assert!(!policy.validate_frame_url("http://example.com"));
        assert!(!policy.validate_frame_url("javascript:alert(1)"));
        assert!(!policy.validate_frame_url("file:///etc/passwd"));
        assert!(!policy.validate_frame_url("about:config"));
    }

    #[test]
    fn test_suffix_attacks() {
        let policy = PolicyManager::new();
        // POC: evilnotion.so should NOT match notion.so
        assert!(!policy.validate_url("https://evilnotion.so"));
        assert!(!policy.validate_url("https://accounts.google.com.evil.com"));
        assert!(!policy.validate_url("https://appleid.apple.com.attacker.com"));
    }
}
