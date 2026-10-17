use std::fmt;

use thiserror::Error;

use crate::sha1;

pub const ORIGIN: &str = "https://music.youtube.com";
const SAPISID_COOKIES: [&str; 2] = ["SAPISID", "__Secure-3PAPISID"];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("no cookie header found in the pasted text")]
    NoCookie,
    #[error("cookie has no SAPISID; are you signed in on music.youtube.com?")]
    NoSapisid,
    #[error("cookie contains line breaks or control characters; paste it on one line")]
    InvalidCookie,
}

pub struct Session {
    cookie: String,
    sapisid: String,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Session(<redacted>)")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        wipe(&mut self.cookie);
        wipe(&mut self.sapisid);
    }
}

// Best effort: overwrites the bytes before freeing so the secret doesn't linger in freed memory.
pub fn wipe(secret: &mut String) {
    let mut bytes = std::mem::take(secret).into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

impl Session {
    pub fn from_cookie(cookie: &str) -> Result<Self, AuthError> {
        let cookie = cookie.trim();
        if cookie.chars().any(char::is_control) {
            return Err(AuthError::InvalidCookie);
        }
        let sapisid = SAPISID_COOKIES
            .iter()
            .find_map(|name| cookie_value(cookie, name).filter(|value| !value.is_empty()))
            .ok_or(AuthError::NoSapisid)?;
        Ok(Self {
            sapisid: sapisid.to_owned(),
            cookie: cookie.to_owned(),
        })
    }

    pub fn from_pasted_headers(text: &str) -> Result<Self, AuthError> {
        let cookie = text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim().eq_ignore_ascii_case("cookie").then_some(value)
            })
            .or_else(|| {
                let line = text.trim();
                (!line.contains('\n') && line.contains('=')).then_some(line)
            })
            .ok_or(AuthError::NoCookie)?;
        Self::from_cookie(cookie)
    }

    pub fn cookie(&self) -> &str {
        &self.cookie
    }

    pub fn authorization_for(&self, origin: &str, unix_secs: u64) -> String {
        let hash = sha1::hex_digest(format!("{unix_secs} {} {origin}", self.sapisid).as_bytes());
        format!("SAPISIDHASH {unix_secs}_{hash}")
    }
}

fn cookie_value<'a>(cookie: &'a str, name: &str) -> Option<&'a str> {
    cookie
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const COOKIE: &str = "HSID=h; SAPISID=abc/def; __Secure-3PAPISID=other";

    #[test]
    fn parses_cookie_line_from_pasted_headers() {
        let text = format!("accept: */*\nCookie: {COOKIE}\nuser-agent: x\n");
        let session = Session::from_pasted_headers(&text).unwrap();
        assert_eq!(session.cookie(), COOKIE);
        assert_eq!(session.sapisid, "abc/def");
    }

    #[test]
    fn accepts_bare_cookie_string() {
        assert!(Session::from_pasted_headers(COOKIE).is_ok());
    }

    #[test]
    fn falls_back_to_secure_papisid() {
        let session = Session::from_cookie("a=1; __Secure-3PAPISID=xyz").unwrap();
        assert_eq!(session.sapisid, "xyz");
    }

    #[test]
    fn does_not_match_cookie_name_suffix() {
        assert_eq!(
            Session::from_cookie("XSAPISID=nope").unwrap_err(),
            AuthError::NoSapisid
        );
    }

    #[test]
    fn rejects_multiline_text_without_cookie_line() {
        let text = "referer: https://x/?a=b\nSAPISID=abc";
        assert_eq!(
            Session::from_pasted_headers(text).unwrap_err(),
            AuthError::NoCookie
        );
    }

    #[test]
    fn rejects_control_characters_in_cookie() {
        assert_eq!(
            Session::from_cookie("SAPISID=a\tb").unwrap_err(),
            AuthError::InvalidCookie
        );
    }

    #[test]
    fn rejects_empty_sapisid() {
        assert_eq!(
            Session::from_cookie("SAPISID=; HSID=h").unwrap_err(),
            AuthError::NoSapisid
        );
    }

    #[test]
    fn rejects_text_without_cookie() {
        assert_eq!(
            Session::from_pasted_headers("accept: */*").unwrap_err(),
            AuthError::NoCookie
        );
        assert_eq!(
            Session::from_pasted_headers("").unwrap_err(),
            AuthError::NoCookie
        );
    }

    #[test]
    fn authorization_matches_sapisidhash_scheme() {
        let session = Session::from_cookie("SAPISID=abc").unwrap();
        let expected = sha1::hex_digest(b"1700000000 abc https://music.youtube.com");
        assert_eq!(
            session.authorization_for(ORIGIN, 1_700_000_000),
            format!("SAPISIDHASH 1700000000_{expected}")
        );
    }

    #[test]
    fn wipe_empties_the_secret() {
        let mut secret = String::from("SAPISID=abc");
        wipe(&mut secret);
        assert!(secret.is_empty());
    }

    #[test]
    fn debug_output_hides_secrets() {
        let session = Session::from_cookie(COOKIE).unwrap();
        assert!(!format!("{session:?}").contains("abc"));
    }
}
