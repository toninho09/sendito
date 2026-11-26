//! Template functions: `{{ $uuidBytes(user_id) }}`.
//!
//! A call is `$name(` + arguments separated by commas + `)`. An argument is a
//! `'literal'` (it ends at the next `'`), a number (`8`, `-1`), a variable
//! name or another call.
//!
//! To add a function, write it here, add a row to [`FUNCS`] and a row to the
//! "Functions" table of `FORMAT.md` (a test checks the table).

use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use sha1::{Digest, Sha1};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::script::{digest, hex};
use crate::template::is_name_char;
use crate::vars::{MAX_DEPTH, MAX_RENDERED};

type Apply = fn(&[String]) -> Result<String, String>;

/// One template function.
pub struct Func {
    /// The name with its `$`, as written in a template.
    pub name: &'static str,
    /// The number of arguments.
    pub arity: usize,
    /// The value is random: the scope keeps the first value of each call text
    /// for the whole send, as for `$uuid`.
    pub random: bool,
    /// Gets the rendered arguments (`arity` of them); an error leaves the
    /// placeholder unresolved.
    pub apply: Apply,
}

impl Func {
    const fn new(name: &'static str, arity: usize, apply: Apply) -> Self {
        Self {
            name,
            arity,
            random: false,
            apply,
        }
    }

    const fn random(name: &'static str, arity: usize, apply: Apply) -> Self {
        Self {
            name,
            arity,
            random: true,
            apply,
        }
    }
}

/// `$default(name, fallback)`: the fallback when the first argument does not
/// resolve. The scope runs it, because an unresolved argument stops any other
/// call.
pub const DEFAULT: &str = "$default";

/// The prefix of a call to a function of the Lua catalog: `$lua.name(...)`.
pub const LUA: &str = "$lua.";

/// Every template function. Bytes come out as standard base64: the JSON form
/// of a protobuf `bytes` field.
pub const FUNCS: &[Func] = &[
    // IDs and bytes
    Func::new("$uuidBytes", 1, |a| {
        uuid_parse(arg(a, 0)).map(|b| STANDARD.encode(b))
    }),
    Func::new("$uuidFromBytes", 1, |a| {
        let b: [u8; 16] = base64_decode(arg(a, 0))?
            .try_into()
            .map_err(|b: Vec<u8>| format!("{} bytes are not a UUID (16 bytes)", b.len()))?;
        Ok(uuid_text(&b))
    }),
    Func::new("$uuidV5", 2, |a| {
        let mut h = Sha1::new();
        h.update(uuid_parse(arg(a, 0))?);
        h.update(arg(a, 1).as_bytes());
        let mut b = [0u8; 16];
        b.copy_from_slice(&h.finalize()[..16]);
        Ok(uuid_text(&with_version(b, 5)))
    }),
    Func::random("$uuidV7", 0, |_| {
        let mut b: [u8; 16] = fastrand::u128(..).to_le_bytes();
        b[..6].copy_from_slice(&now_ms().to_be_bytes()[2..]);
        Ok(uuid_text(&with_version(b, 7)))
    }),
    Func::random("$ulid", 0, |_| {
        const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let v = (u128::from(now_ms()) << 80) | (fastrand::u128(..) >> 48);
        Ok((0..26)
            .rev()
            .map(|i| char::from(CROCKFORD[((v >> (5 * i)) & 31) as usize]))
            .collect())
    }),
    Func::new("$hexBytes", 1, |a| {
        hex_decode(arg(a, 0)).map(|b| STANDARD.encode(b))
    }),
    Func::new("$bytesHex", 1, |a| {
        base64_decode(arg(a, 0)).map(|b| hex(&b))
    }),
    // Encoding
    Func::new("$base64", 1, |a| Ok(STANDARD.encode(arg(a, 0)))),
    Func::new("$base64Url", 1, |a| Ok(URL_SAFE_NO_PAD.encode(arg(a, 0)))),
    Func::new("$base64Decode", 1, |a| {
        String::from_utf8(base64_decode(arg(a, 0))?)
            .map_err(|_| "the bytes are not UTF-8 text".to_string())
    }),
    Func::new("$hex", 1, |a| Ok(hex(arg(a, 0).as_bytes()))),
    Func::new("$urlEncode", 1, |a| {
        Ok(crate::resolve::query_encode(arg(a, 0)))
    }),
    // Hashes and signatures
    Func::new("$md5", 1, |a| digest("md5", arg(a, 0).as_bytes())),
    Func::new("$sha1", 1, |a| digest("sha1", arg(a, 0).as_bytes())),
    Func::new("$sha256", 1, |a| digest("sha256", arg(a, 0).as_bytes())),
    Func::new("$sha512", 1, |a| digest("sha512", arg(a, 0).as_bytes())),
    Func::new("$hmacSha256", 2, |a| Ok(hex(&hmac_sha256(a)))),
    Func::new("$hmacSha256Base64", 2, |a| {
        Ok(STANDARD.encode(hmac_sha256(a)))
    }),
    // Time
    Func::new("$timeAdd", 2, |a| {
        let t = iso_parse(arg(a, 0))?;
        let d = duration(arg(a, 1))?;
        iso_format(t.checked_add(d).ok_or("the time is out of range")?)
    }),
    Func::new("$isoFromUnix", 1, |a| {
        let t =
            OffsetDateTime::from_unix_timestamp(number(arg(a, 0))?).map_err(|e| e.to_string())?;
        iso_format(t)
    }),
    Func::new("$unixFromIso", 1, |a| {
        Ok(iso_parse(arg(a, 0))?.unix_timestamp().to_string())
    }),
    // Text and JSON
    Func::new("$upper", 1, |a| Ok(arg(a, 0).to_uppercase())),
    Func::new("$lower", 1, |a| Ok(arg(a, 0).to_lowercase())),
    Func::new("$trim", 1, |a| Ok(arg(a, 0).trim().to_string())),
    Func::new("$len", 1, |a| Ok(arg(a, 0).len().to_string())),
    Func::new("$jsonEscape", 1, |a| Ok(json_escape(arg(a, 0)))),
    Func::new("$jsonString", 1, |a| {
        serde_json::to_string(arg(a, 0)).map_err(|e| e.to_string())
    }),
    Func::new("$replace", 3, |a| {
        if arg(a, 1).is_empty() {
            return Err("the text to replace is empty".into());
        }
        Ok(arg(a, 0).replace(arg(a, 1), arg(a, 2)))
    }),
    Func::new("$substr", 3, |a| {
        let (start, len) = (size(arg(a, 1))?, size(arg(a, 2))?);
        Ok(arg(a, 0).chars().skip(start).take(len).collect())
    }),
    Func::new("$padLeft", 3, |a| {
        let mut pad = arg(a, 2).chars();
        let (Some(c), None) = (pad.next(), pad.next()) else {
            return Err("the pad must be one character".into());
        };
        let s = arg(a, 0);
        let missing = size(arg(a, 1))?.saturating_sub(s.chars().count());
        Ok(std::iter::repeat_n(c, missing).chain(s.chars()).collect())
    }),
    Func::new(DEFAULT, 2, |a| Ok(arg(a, 0).to_string())),
    // Random
    Func::random("$randomInt", 2, |a| {
        let (min, max): (i64, i64) = (number(arg(a, 0))?, number(arg(a, 1))?);
        if min > max {
            return Err(format!("{min} is more than {max}"));
        }
        Ok(fastrand::i64(min..=max).to_string())
    }),
    Func::random("$randomHex", 1, |a| {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        Ok((0..size(arg(a, 0))?)
            .map(|_| char::from(HEX[fastrand::usize(..16)]))
            .collect())
    }),
    Func::random("$randomString", 1, |a| {
        Ok((0..size(arg(a, 0))?)
            .map(|_| fastrand::alphanumeric())
            .collect())
    }),
];

/// The function named `name` (with its `$`).
pub fn find(name: &str) -> Option<&'static Func> {
    FUNCS.iter().find(|f| f.name == name)
}

/// Argument `i` (empty when there is none).
fn arg(a: &[String], i: usize) -> &str {
    a.get(i).map_or("", String::as_str)
}

fn number<T: FromStr>(s: &str) -> Result<T, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{s:?} is not a number"))
}

/// A count or a length, at most [`MAX_RENDERED`].
fn size(s: &str) -> Result<usize, String> {
    let n: usize = number(s)?;
    if n > MAX_RENDERED {
        return Err(format!("{n} is more than {MAX_RENDERED}"));
    }
    Ok(n)
}

/// `s` as it is written inside a JSON string, without the quotes.
pub fn json_escape(s: &str) -> String {
    serde_json::to_string(s)
        .map(|j| j[1..j.len() - 1].to_string())
        .unwrap_or_default()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{s:?} is not an even number of hex digits"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Standard or URL-safe base64, with or without padding.
fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    STANDARD
        .decode(s)
        .or_else(|_| URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')))
        .map_err(|_| format!("{s:?} is not base64"))
}

/// The 16 bytes of a UUID: hyphens are optional, any case.
fn uuid_parse(s: &str) -> Result<Vec<u8>, String> {
    let digits: String = s.trim().chars().filter(|c| *c != '-').collect();
    if digits.len() != 32 {
        return Err(format!("{s:?} is not a UUID"));
    }
    hex_decode(&digits)
}

/// A UUID in its text form: lowercase, with hyphens.
pub(crate) fn uuid_text(b: &[u8; 16]) -> String {
    let h = hex(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Sets the version and the RFC 4122 variant bits.
pub(crate) fn with_version(mut b: [u8; 16], version: u8) -> [u8; 16] {
    b[6] = (b[6] & 0x0f) | (version << 4);
    b[8] = (b[8] & 0x3f) | 0x80;
    b
}

/// Unix time in milliseconds.
fn now_ms() -> u64 {
    u64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

fn hmac_sha256(a: &[String]) -> Vec<u8> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, arg(a, 0).as_bytes());
    ring::hmac::sign(&key, arg(a, 1).as_bytes())
        .as_ref()
        .to_vec()
}

fn iso_parse(s: &str) -> Result<OffsetDateTime, String> {
    OffsetDateTime::parse(s.trim(), &Rfc3339).map_err(|_| format!("{s:?} is not an RFC 3339 time"))
}

/// RFC 3339 in UTC, as `$isoTimestamp`.
fn iso_format(t: OffsetDateTime) -> Result<String, String> {
    t.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|e| e.to_string())
}

/// A duration such as `90s`, `1h30m` or `-2d`: an optional sign, then
/// numbers with a unit (`ms`, `s`, `m`, `h`, `d`).
fn duration(s: &str) -> Result<time::Duration, String> {
    const UNITS: [(&str, i64); 5] = [
        ("ms", 1),
        ("s", 1_000),
        ("m", 60_000),
        ("h", 3_600_000),
        ("d", 86_400_000),
    ];
    let bad = || format!("{s:?} is not a duration such as 1h30m or -15s");
    let t = s.trim();
    let (negative, mut rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if rest.is_empty() {
        return Err(bad());
    }
    let mut ms: i64 = 0;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let n: i64 = rest[..digits].parse().map_err(|_| bad())?;
        rest = &rest[digits..];
        let (unit, unit_ms) = UNITS
            .into_iter()
            .find(|(u, _)| rest.starts_with(u))
            .ok_or_else(bad)?;
        rest = &rest[unit.len()..];
        ms = n
            .checked_mul(unit_ms)
            .and_then(|v| ms.checked_add(v))
            .ok_or_else(bad)?;
    }
    Ok(time::Duration::milliseconds(if negative {
        -ms
    } else {
        ms
    }))
}

/// An argument of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg<'a> {
    /// A `'literal'` without the quotes, or a number.
    Text(&'a str),
    /// A variable name, such as `user_id` or `$uuid`.
    Name(&'a str),
    Call(Call<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call<'a> {
    pub name: &'a str,
    pub args: Vec<Arg<'a>>,
    /// The call as written, such as `$randomHex(8)`.
    pub text: &'a str,
}

/// Parses a placeholder name such as `$f(a, 'b', $g(c))`. `None` when it is
/// not a call or the calls nest deeper than [`MAX_DEPTH`].
pub fn parse(text: &str) -> Option<Call<'_>> {
    let mut p = Parser { s: text, i: 0 };
    let c = p.call(0)?;
    p.skip_spaces();
    (p.i == text.len()).then_some(c)
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn skip_spaces(&mut self) {
        while self.peek() == Some(b' ') {
            self.i += 1;
        }
    }

    fn name(&mut self) -> Option<&'a str> {
        let start = self.i;
        if self.peek() == Some(b'$') {
            self.i += 1;
        }
        while self.peek().is_some_and(is_name_char) {
            self.i += 1;
        }
        let n = &self.s[start..self.i];
        (!n.is_empty() && n != "$").then_some(n)
    }

    fn call(&mut self, depth: usize) -> Option<Call<'a>> {
        let start = self.i;
        let name = self.name()?;
        if depth >= MAX_DEPTH || !name.starts_with('$') || self.peek() != Some(b'(') {
            return None;
        }
        self.i += 1;
        let mut args = Vec::new();
        self.skip_spaces();
        if self.peek() != Some(b')') {
            loop {
                self.skip_spaces();
                args.push(self.arg(depth)?);
                self.skip_spaces();
                match self.peek()? {
                    b',' => self.i += 1,
                    b')' => break,
                    _ => return None,
                }
            }
        }
        self.i += 1;
        Some(Call {
            name,
            args,
            text: &self.s[start..self.i],
        })
    }

    fn arg(&mut self, depth: usize) -> Option<Arg<'a>> {
        if self.peek() == Some(b'\'') {
            let start = self.i + 1;
            let len = self.s[start..].find('\'')?;
            self.i = start + len + 1;
            return Some(Arg::Text(&self.s[start..start + len]));
        }
        let start = self.i;
        let name = self.name()?;
        if self.peek() == Some(b'(') {
            self.i = start;
            return self.call(depth + 1).map(Arg::Call);
        }
        let digits = name.strip_prefix('-').unwrap_or(name);
        if !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) {
            return Some(Arg::Text(name));
        }
        Some(Arg::Name(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(name: &str, args: &[&str]) -> Result<String, String> {
        let f = find(name).ok_or("unknown")?;
        assert_eq!(f.arity, args.len(), "{name}");
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        (f.apply)(&args)
    }

    #[test]
    fn every_function() {
        let uuid = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";
        let fox = "The quick brown fox jumps over the lazy dog";
        let dns = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let cases: &[(&str, &[&str], &str)] = &[
            ("$uuidBytes", &[uuid], "G04oui+hEdKIPwAW08ykJw=="),
            (
                "$uuidBytes",
                &["1B4E28BA2FA111D2883F0016D3CCA427"],
                "G04oui+hEdKIPwAW08ykJw==",
            ),
            ("$uuidFromBytes", &["G04oui+hEdKIPwAW08ykJw=="], uuid),
            ("$uuidFromBytes", &["G04oui-hEdKIPwAW08ykJw"], uuid),
            (
                "$uuidV5",
                &[dns, "python.org"],
                "886313e1-3b8a-5372-9b90-0c9aee199e5d",
            ),
            ("$hexBytes", &["0001ff"], "AAH/"),
            ("$bytesHex", &["AAH/"], "0001ff"),
            ("$base64", &["hé?"], "aMOpPw=="),
            ("$base64Url", &["hé?"], "aMOpPw"),
            ("$base64Decode", &["aMOpPw=="], "hé?"),
            ("$hex", &["hé"], "68c3a9"),
            ("$urlEncode", &["a b&c"], "a%20b%26c"),
            ("$md5", &["abc"], "900150983cd24fb0d6963f7d28e17f72"),
            (
                "$sha1",
                &["abc"],
                "a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (
                "$sha256",
                &["abc"],
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                "$sha512",
                &[""],
                "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
            ),
            (
                "$hmacSha256",
                &["key", fox],
                "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8",
            ),
            (
                "$hmacSha256Base64",
                &["key", fox],
                "97yD9DBThCSxMpjmqm+xQ+9NWaFJRhdZl0edvC0aPNg=",
            ),
            (
                "$timeAdd",
                &["2024-01-31T23:00:00Z", "1h30m"],
                "2024-02-01T00:30:00Z",
            ),
            (
                "$timeAdd",
                &["2024-01-01T00:00:00+02:00", "-1d"],
                "2023-12-30T22:00:00Z",
            ),
            (
                "$timeAdd",
                &["2024-01-01T00:00:00Z", "+1500ms"],
                "2024-01-01T00:00:01.5Z",
            ),
            ("$isoFromUnix", &["0"], "1970-01-01T00:00:00Z"),
            ("$unixFromIso", &["1970-01-01T00:00:10+01:00"], "-3590"),
            ("$upper", &["aé"], "AÉ"),
            ("$lower", &["AÉ"], "aé"),
            ("$trim", &["  x "], "x"),
            ("$len", &["hé"], "3"),
            ("$jsonEscape", &["a\"b\\c\n"], "a\\\"b\\\\c\\n"),
            ("$jsonString", &["a\"b"], "\"a\\\"b\""),
            ("$replace", &["a-b-c", "-", "+"], "a+b+c"),
            ("$substr", &["héllo", "1", "3"], "éll"),
            ("$substr", &["abc", "2", "10"], "c"),
            ("$padLeft", &["42", "5", "0"], "00042"),
            ("$padLeft", &["123456", "5", "0"], "123456"),
            ("$default", &["a", "b"], "a"),
            ("$randomInt", &["7", "7"], "7"),
            ("$randomHex", &["0"], ""),
            ("$randomString", &["0"], ""),
        ];
        for (name, args, want) in cases {
            assert_eq!(run(name, args).as_deref(), Ok(*want), "{name}({args:?})");
        }
        // A random function with no argument is in `random_values`.
        for f in FUNCS {
            assert!(
                (f.random && f.arity == 0) || cases.iter().any(|(n, _, _)| *n == f.name),
                "{} has no case",
                f.name
            );
        }
    }

    #[test]
    fn random_values() {
        let v7 = run("$uuidV7", &[]).unwrap();
        assert_eq!(v7.len(), 36);
        assert_eq!(&v7[14..15], "7");
        assert!(matches!(&v7[19..20], "8" | "9" | "a" | "b"));
        let ms = (u64::from_str_radix(&v7[..8], 16).unwrap() << 16)
            | u64::from_str_radix(&v7[9..13], 16).unwrap();
        assert!(ms.abs_diff(now_ms()) < 60_000);
        let ulid = run("$ulid", &[]).unwrap();
        assert_eq!(ulid.len(), 26);
        assert!(
            ulid.bytes()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        );
        assert!(ulid.as_bytes()[0] <= b'7');
        let n: i64 = run("$randomInt", &["-3", "3"]).unwrap().parse().unwrap();
        assert!((-3..=3).contains(&n));
        let h = run("$randomHex", &["32"]).unwrap();
        assert!(h.len() == 32 && h.bytes().all(|c| c.is_ascii_hexdigit()));
        let s = run("$randomString", &["20"]).unwrap();
        assert!(s.len() == 20 && s.bytes().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn bad_input_is_an_error() {
        let bad: &[(&str, &[&str])] = &[
            ("$uuidBytes", &["abc"]),
            ("$uuidBytes", &["1b4e28ba-2fa1-11d2-883f-0016d3cca42z"]),
            ("$uuidFromBytes", &["AAH/"]),
            ("$uuidV5", &["dns", "x"]),
            ("$hexBytes", &["abc"]),
            ("$hexBytes", &["zz"]),
            ("$bytesHex", &["%%"]),
            ("$base64Decode", &["/w=="]),
            ("$timeAdd", &["yesterday", "1h"]),
            ("$timeAdd", &["2024-01-01T00:00:00Z", "1x"]),
            ("$timeAdd", &["2024-01-01T00:00:00Z", "h"]),
            ("$timeAdd", &["2024-01-01T00:00:00Z", "-"]),
            ("$timeAdd", &["2024-01-01T00:00:00Z", "99999999999999999d"]),
            ("$isoFromUnix", &["now"]),
            ("$unixFromIso", &["2024-01-01"]),
            ("$replace", &["abc", "", "x"]),
            ("$substr", &["abc", "-1", "2"]),
            ("$padLeft", &["1", "3", "00"]),
            ("$padLeft", &["1", "99999999", "0"]),
            ("$randomInt", &["5", "1"]),
            ("$randomHex", &["x"]),
        ];
        for (name, args) in bad {
            assert!(run(name, args).is_err(), "{name}({args:?})");
        }
    }

    #[test]
    fn names_are_unique_and_documented() {
        let format = include_str!("../../../FORMAT.md");
        for (i, f) in FUNCS.iter().enumerate() {
            assert!(f.name.starts_with('$') && f.name.len() > 1);
            assert!(FUNCS[..i].iter().all(|g| g.name != f.name), "{}", f.name);
            assert!(
                format.contains(&format!("`{}(", f.name)),
                "{} is not in the FORMAT.md functions table",
                f.name
            );
        }
    }

    #[test]
    fn parses_calls() {
        let text = "$f( a , 'b, }}' , $g($uuid), -12 )";
        assert_eq!(
            parse(text),
            Some(Call {
                name: "$f",
                args: vec![
                    Arg::Name("a"),
                    Arg::Text("b, }}"),
                    Arg::Call(Call {
                        name: "$g",
                        args: vec![Arg::Name("$uuid")],
                        text: "$g($uuid)",
                    }),
                    Arg::Text("-12"),
                ],
                text,
            })
        );
        assert_eq!(
            parse("$f()"),
            Some(Call {
                name: "$f",
                args: vec![],
                text: "$f()",
            })
        );
        for bad in [
            "f(a)", "$f", "$f(a", "$f(a b)", "$f('a)", "$f(a,)", "$f(a))", "$f(a) x",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        let deep = format!("{}x{}", "$f(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
        assert!(parse(&deep).is_some());
        let too_deep = format!(
            "{}x{}",
            "$f(".repeat(MAX_DEPTH + 1),
            ")".repeat(MAX_DEPTH + 1)
        );
        assert_eq!(parse(&too_deep), None);
    }
}
