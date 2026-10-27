//! 20b: web addresses for your browser: "go to github.com", "search youtube for lofi music". The
//! browser goes online, not Siva: nothing here opens a connection (the `no_network` test lets
//! this one file hold web addresses, and still bans network clients and sockets everywhere).
//! Pure and tested.

/// A web address to open: `https://` added when there's no scheme; only http(s); no spaces.
pub fn web_address(text: &str) -> Option<String> {
    let t = text.trim().trim_end_matches('.');
    if t.is_empty() || t.contains(char::is_whitespace) || t.len() > 300 {
        return None;
    }
    let url = if t.starts_with("http://") || t.starts_with("https://") { t.to_string() } else { format!("https://{t}") };
    let host = url.split("://").nth(1)?.split(['/', '?', '#']).next()?;
    (host.contains('.') && !host.starts_with('.') && !host.ends_with('.')).then_some(url)
}

/// 20b: a well-known site's address, for "open youtube" when no app is called that.
pub fn site_for(name: &str) -> Option<&'static str> {
    const SITES: [(&str, &str); 19] = [
        ("youtube", "https://www.youtube.com"), ("gmail", "https://mail.google.com"), ("google", "https://www.google.com"), ("github", "https://github.com"),
        ("wikipedia", "https://www.wikipedia.org"), ("netflix", "https://www.netflix.com"), ("amazon", "https://www.amazon.in"), ("facebook", "https://www.facebook.com"),
        ("instagram", "https://www.instagram.com"), ("twitter", "https://x.com"), ("reddit", "https://www.reddit.com"), ("whatsapp", "https://web.whatsapp.com"),
        ("linkedin", "https://www.linkedin.com"), ("chatgpt", "https://chatgpt.com"), ("claude", "https://claude.ai"), ("google maps", "https://maps.google.com"),
        ("maps", "https://maps.google.com"), ("google drive", "https://drive.google.com"), ("google news", "https://news.google.com"),
    ];
    let n = name.trim().to_lowercase();
    // 21f: "google news in firefox": the site, in the browser it names.
    let n = [" in firefox", " on firefox", " in chrome", " in the browser", " in browser", " in my browser"].iter().find_map(|e| n.strip_suffix(e)).unwrap_or(&n).trim();
    SITES.iter().find(|(k, _)| *k == n).map(|(_, u)| *u)
}

/// Percent-encoding for a search box.
pub fn encode_query(q: &str) -> String {
    q.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            b => format!("%{b:02X}"),
        })
        .collect()
}

/// A search in the browser, on the web or a site.
pub fn search_url(site: &str, query: &str) -> String {
    let q = encode_query(query.trim());
    match site {
        "youtube" => format!("https://www.youtube.com/results?search_query={q}"),
        "wikipedia" => format!("https://en.wikipedia.org/w/index.php?search={q}"),
        "maps" => format!("https://www.google.com/maps/search/{q}"),
        "images" => format!("https://www.google.com/search?tbm=isch&q={q}"),
        "news" => format!("https://news.google.com/search?q={q}"),
        "github" => format!("https://github.com/search?q={q}"),
        _ => format!("https://www.google.com/search?q={q}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses() {
        assert_eq!(site_for("YouTube"), Some("https://www.youtube.com"));
        assert_eq!(site_for("gmail"), Some("https://mail.google.com"));
        assert_eq!(site_for("Google News"), Some("https://news.google.com"));
        assert_eq!(site_for("google news in firefox"), Some("https://news.google.com"), "from Siva's log");
        assert_eq!(site_for("firefox"), None);
        assert_eq!(site_for("text editor"), None);
        assert_eq!(web_address("github.com").as_deref(), Some("https://github.com"));
        assert_eq!(web_address("en.wikipedia.org/wiki/Chennai").as_deref(), Some("https://en.wikipedia.org/wiki/Chennai"));
        assert_eq!(web_address("http://example.com").as_deref(), Some("http://example.com"));
        assert_eq!(web_address("javascript:alert(1)"), None, "only http(s)");
        assert_eq!(web_address("file:///etc/passwd"), None);
        assert_eq!(web_address("not a site"), None);
        assert_eq!(web_address("localhost"), None);
        assert_eq!(search_url("youtube", "lofi music"), "https://www.youtube.com/results?search_query=lofi+music");
        assert_eq!(search_url("web", "c++ & rust?"), "https://www.google.com/search?q=c%2B%2B+%26+rust%3F");
        assert_eq!(encode_query("தமிழ்").starts_with('%'), true, "non-ASCII percent-encoded");
    }
}
