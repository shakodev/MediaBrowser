use url::Url;

pub const HOME: &str = "https://www.youtube.com/";

pub fn is_youtube_page(url: &Url) -> bool {
    secure_url(url)
        && matches!(
            url.host_str(),
            Some(
                "youtube.com"
                    | "www.youtube.com"
                    | "m.youtube.com"
                    | "music.youtube.com"
                    | "youtu.be"
            )
        )
}

fn secure_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
}

pub fn is_browser_url(url: &Url) -> bool {
    is_youtube_page(url)
        || (secure_url(url)
            && matches!(
                url.host_str(),
                Some("consent.youtube.com" | "consent.google.com" | "accounts.google.com")
            ))
}

pub fn is_local_url(url: &Url) -> bool {
    let bundled = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http"
            && url.host_str() == Some("tauri.localhost")
            && url.port().is_none());
    let dev = cfg!(debug_assertions)
        && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1420);
    (bundled || dev) && url.username().is_empty() && url.password().is_none()
}

pub fn address_or_search(input: &str) -> Result<Url, String> {
    let input = input.trim();
    if input.is_empty() {
        return Url::parse(HOME).map_err(|e| e.to_string());
    }
    if input.len() > 8192 {
        return Err("The address or search is too long.".into());
    }
    let looks_like_url = input.contains(":")
        || input.starts_with("//")
        || [
            "youtube.com",
            "www.youtube.com",
            "m.youtube.com",
            "music.youtube.com",
            "youtu.be",
        ]
        .iter()
        .any(|host| input == *host || input.starts_with(&format!("{host}/")));
    if looks_like_url {
        let candidate = if input.contains("://") || input.contains(':') {
            input.to_owned()
        } else {
            format!("https://{}", input.trim_start_matches('/'))
        };
        let url = Url::parse(&candidate).map_err(|_| "Enter a valid YouTube URL.".to_string())?;
        if !is_youtube_page(&url) {
            return Err("Use an HTTPS YouTube URL or enter a search.".into());
        }
        Ok(url)
    } else {
        let mut url = Url::parse("https://www.youtube.com/results").map_err(|e| e.to_string())?;
        url.query_pairs_mut().append_pair("search_query", input);
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_youtube_addresses_and_encodes_search() {
        assert_eq!(address_or_search(" ").unwrap().as_str(), HOME);
        assert_eq!(
            address_or_search("youtu.be/abc").unwrap().as_str(),
            "https://youtu.be/abc"
        );
        assert_eq!(
            address_or_search("www.youtube.com/watch?v=abc")
                .unwrap()
                .query(),
            Some("v=abc")
        );
        let url = address_or_search("jazz & piano / café").unwrap();
        assert_eq!(url.query_pairs().next().unwrap().1, "jazz & piano / café");
    }

    #[test]
    fn rejects_privileged_and_deceptive_navigation() {
        for input in [
            "javascript:alert(1)",
            "file:///C:/Windows",
            "http://www.youtube.com/",
            "https://www.youtube.com.evil.test/",
            "https://www.youtube.com@evil.test/",
            "https://user@www.youtube.com/",
            "https://www.youtube.com:8443/",
            "http://tauri.localhost",
            "https://example.com/",
            "data:text/html,test",
        ] {
            assert!(address_or_search(input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn consent_is_allowed_but_never_remembered_as_youtube() {
        let consent = Url::parse("https://consent.youtube.com/m?continue=test").unwrap();
        assert!(is_browser_url(&consent));
        assert!(!is_youtube_page(&consent));
        assert!(!is_browser_url(
            &Url::parse("https://evil.youtube.com/").unwrap()
        ));
        assert!(!is_local_url(&Url::parse(HOME).unwrap()));
    }
}
