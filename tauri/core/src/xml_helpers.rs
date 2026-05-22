//! Faithful port of `app/lib/xml_helpers.rb`. Same input → same output;
//! covered by unit tests against fixtures so changes here can't silently
//! diverge from the Ruby behaviour.

/// Replace the five canonical XML numeric entities (matching the Ruby
/// `unescape_xml`) and then URL-decode `%XX` byte sequences. The byte sequence
/// is interpreted as UTF-8 once fully assembled, which is how the Ruby code
/// reassembles characters like "Crumba%CC%88cher" → "Crumbächer".
pub fn unescape_xml(input: &str) -> String {
    // Step 1: numeric entity substitution. Same five entities the Ruby code
    // handles, in the same order (order matters for &#38; vs &).
    let entity_replaced = input
        .replace("&#60;", "<")
        .replace("&#62;", ">")
        .replace("&#34;", "\"")
        .replace("&#38;", "&")
        .replace("&#39;", "'");

    // Step 2: URL-decode %XX sequences into raw bytes, then re-interpret the
    // whole result as UTF-8. We can't decode percent-escapes one char at a time
    // because a single non-ASCII codepoint is encoded as multiple %XX bytes.
    let bytes = entity_replaced.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) =
                (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2]))
            {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }

    // If the bytes don't form valid UTF-8 we fall back to lossy conversion so
    // the user at least sees something rather than the function failing. This
    // matches Ruby's force_encoding which doesn't validate.
    match String::from_utf8(out) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(&e.into_bytes()).into_owned(),
    }
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Strip the `file://` (and optional `localhost/`) prefix from an iTunes
/// Location URL, matching `strip_url_file_path_starting` in the Ruby code.
pub fn strip_url_file_path_starting(input: &str) -> String {
    // `file://...` URL forms:
    //   file:///Users/x          — empty authority, Unix abs path
    //   file://localhost/Users/x — localhost authority, Unix abs path
    //   file:///C:/Users/x       — Windows drive letter
    //   file://localhost/C:/Users/x — Windows w/ explicit authority
    //
    // After stripping scheme + authority we keep the leading `/` so
    // Unix absolute paths stay absolute. For Windows we recognise the
    // `/X:/...` pattern (X is a letter) and strip that leading slash so
    // `C:\Users\...` style paths round-trip correctly through fs APIs.
    let Some(rest) = input.strip_prefix("file://") else {
        return input.to_string();
    };
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
    {
        // Windows: "/C:/Users/..." → "C:/Users/..."
        return path[1..].to_string();
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_basic_entities() {
        assert_eq!(unescape_xml("&#60;hello&#62;"), "<hello>");
        assert_eq!(unescape_xml("a &#38; b"), "a & b");
        assert_eq!(unescape_xml("&#34;quoted&#34;"), "\"quoted\"");
        assert_eq!(unescape_xml("it&#39;s"), "it's");
    }

    #[test]
    fn unescape_percent_utf8_byte_sequence() {
        // From the Ruby comment: "Crumba%CC%88cher" -> "Crumbächer"
        // (CC 88 is the UTF-8 byte sequence for U+0308 combining diaeresis,
        // producing the *decomposed* form "a" + COMBINING DIAERESIS — Ruby
        // produces the same since it just decodes raw bytes.)
        assert_eq!(unescape_xml("Crumba%CC%88cher"), "Crumba\u{0308}cher");
    }

    #[test]
    fn unescape_passthrough_for_plain_ascii() {
        assert_eq!(unescape_xml("Hello, World!"), "Hello, World!");
    }

    #[test]
    fn unescape_preserves_unmatched_percent() {
        // A stray % that isn't followed by two hex digits should pass through.
        assert_eq!(unescape_xml("100% pure"), "100% pure");
    }

    #[test]
    fn strip_file_url_basic() {
        assert_eq!(
            strip_url_file_path_starting("file:///Users/andrew/Music/x.mp3"),
            "/Users/andrew/Music/x.mp3"
        );
    }

    #[test]
    fn strip_file_url_with_localhost() {
        // The leading slash must survive — it's part of the absolute path,
        // not part of the authority delimiter.
        assert_eq!(
            strip_url_file_path_starting("file://localhost/Users/andrew/x.mp3"),
            "/Users/andrew/x.mp3"
        );
    }

    #[test]
    fn strip_file_url_windows_drive() {
        assert_eq!(
            strip_url_file_path_starting("file://localhost/C:/Users/Andrew/x.mp3"),
            "C:/Users/Andrew/x.mp3"
        );
        assert_eq!(
            strip_url_file_path_starting("file:///D:/music/x.mp3"),
            "D:/music/x.mp3"
        );
    }

    #[test]
    fn strip_passthrough_for_non_file_url() {
        assert_eq!(strip_url_file_path_starting("/local/path.mp3"), "/local/path.mp3");
    }
}
