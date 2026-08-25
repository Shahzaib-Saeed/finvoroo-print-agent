const TOKEN_BYTES: usize = 32;

pub fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    if getrandom::getrandom(&mut bytes).is_err() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((now >> ((i % 16) * 8)) as u8).wrapping_add(i as u8);
        }
    }
    hex::encode(bytes)
}

pub fn tokens_match(expected: &str, provided: &str) -> bool {
    let expected = expected.trim().as_bytes();
    let provided = provided.trim().as_bytes();
    if expected.is_empty() || expected.len() != provided.len() {
        return false;
    }
    expected
        .iter()
        .zip(provided.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

pub fn origin_is_allowed(origin: &str) -> bool {
    let Ok(url) = http::Uri::try_from(origin.trim()) else {
        return false;
    };
    let host = url.host().unwrap_or("");
    let scheme = url.scheme_str().unwrap_or("");

    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return scheme == "http" || scheme == "https";
    }
    if host == "finvoroo.com" || host.ends_with(".finvoroo.com") {
        return scheme == "https";
    }
    false
}

pub fn origin_from_headers(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get(http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

pub fn token_from_headers(headers: &http::HeaderMap) -> Option<String> {
    if let Some(value) = headers.get("x-finvoroo-print-token") {
        if let Ok(s) = value.to_str() {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    if let Some(value) = headers.get(http::header::AUTHORIZATION) {
        if let Ok(s) = value.to_str() {
            let s = s.trim();
            if let Some(rest) = s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")) {
                let rest = rest.trim();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_match_constant_time() {
        assert!(tokens_match("abc", "abc"));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("abc", "ab"));
        assert!(!tokens_match("", "abc"));
    }

    #[test]
    fn origin_allowlist() {
        assert!(origin_is_allowed("http://localhost:5173"));
        assert!(origin_is_allowed("http://127.0.0.1:4173"));
        assert!(origin_is_allowed("https://app.finvoroo.com"));
        assert!(origin_is_allowed("https://finvoroo.com"));
        assert!(!origin_is_allowed("https://evil.example"));
        assert!(!origin_is_allowed("http://finvoroo.com"));
        assert!(!origin_is_allowed("https://finvoroo.com.evil.test"));
    }

    #[test]
    fn token_from_custom_and_bearer_headers() {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-finvoroo-print-token", "abc123".parse().unwrap());
        assert_eq!(token_from_headers(&headers).as_deref(), Some("abc123"));

        headers.clear();
        headers.insert(http::header::AUTHORIZATION, "Bearer xyz".parse().unwrap());
        assert_eq!(token_from_headers(&headers).as_deref(), Some("xyz"));
    }
}
