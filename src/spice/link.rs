//! SPICE transport (TCP/unix), link handshake and RSA-OAEP ticket authentication.
//!
//! Ported from the working minimal client in `docs/spice-html5/spiceconn.js`
//! (`send_hdr`, ticket state machine) and `docs/spice-html5/ticket.js`
//! (`create_rsa_from_mb`, `RSA_padding_add_PKCS1_OAEP`).

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use rsa::oaep::Oaep;
use rsa::{BigUint, RsaPublicKey};
use sha1::Sha1;

use super::proto::{self, Reader};

/// A blocking connection to one SPICE channel endpoint.
pub enum Stream {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Stream {
    /// Connect over TCP and set the read timeout used by the handshake.
    pub fn connect_tcp(
        host: &str,
        port: u16,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Stream, String> {
        let addrs = (host, port)
            .to_socket_addrs()
            .map_err(|e| format!("cannot resolve SPICE host {}:{}: {}", host, port, e))?;
        let mut last_err: Option<std::io::Error> = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, connect_timeout) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(read_timeout))
                        .map_err(|e| format!("cannot set SPICE socket read timeout: {}", e))?;
                    return Ok(Stream::Tcp(stream));
                }
                Err(e) => last_err = Some(e),
            }
        }
        match last_err {
            Some(e) => Err(format!(
                "cannot connect to SPICE server at {}:{}: {}",
                host, port, e
            )),
            None => Err(format!("no address found for SPICE host {}:{}", host, port)),
        }
    }

    /// Connect to a local SPICE unix socket.
    ///
    /// `connect_timeout` is accepted for symmetry with [`Stream::connect_tcp`];
    /// `UnixStream::connect` to a local path either succeeds or fails
    /// immediately, so there is nothing to time out.
    pub fn connect_unix(
        path: &str,
        read_timeout: Duration,
        _connect_timeout: Duration,
    ) -> Result<Stream, String> {
        let stream = UnixStream::connect(path)
            .map_err(|e| format!("cannot connect to SPICE unix socket {}: {}", path, e))?;
        stream
            .set_read_timeout(Some(read_timeout))
            .map_err(|e| format!("cannot set SPICE socket read timeout: {}", e))?;
        Ok(Stream::Unix(stream))
    }

    pub fn set_read_timeout(&self, timeout: Duration) -> Result<(), String> {
        match self {
            Stream::Tcp(s) => s
                .set_read_timeout(Some(timeout))
                .map_err(|e| format!("cannot set SPICE socket read timeout: {}", e)),
            Stream::Unix(s) => s
                .set_read_timeout(Some(timeout))
                .map_err(|e| format!("cannot set SPICE socket read timeout: {}", e)),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            Stream::Unix(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            Stream::Unix(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            Stream::Unix(s) => s.flush(),
        }
    }
}

/// Parsed `SpiceLinkReply` (the server's answer to our `SpiceLinkMess`).
pub struct LinkReply {
    pub error: u32,
    /// Raw `pub_key[162]` field (a DER SubjectPublicKeyInfo, zero padded).
    pub pub_key: Vec<u8>,
    pub common_caps: Vec<u32>,
    pub channel_caps: Vec<u32>,
}

/// Build the 16-byte `SpiceLinkHeader` that precedes a `SpiceLinkMess`.
pub fn build_link_header(body_len: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&proto::SPICE_MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&proto::SPICE_VERSION_MAJOR.to_le_bytes());
    out[8..12].copy_from_slice(&proto::SPICE_VERSION_MINOR.to_le_bytes());
    out[12..16].copy_from_slice(&body_len.to_le_bytes());
    out
}

/// Build a `SpiceLinkMess` body, including the capability words.
///
/// Layout (little-endian): connection_id u32, channel_type u8, channel_id u8,
/// num_common_caps u32, num_channel_caps u32, caps_offset u32 (= 18), then the
/// common capability word(s) followed by the channel capability word(s).
pub fn build_link_mess(connection_id: u32, channel_type: u8, channel_caps: &[u32]) -> Vec<u8> {
    // PROTOCOL_AUTH_SELECTION lets us pick SPICE ticket auth; MINI_HEADER makes
    // every later message use SpiceMiniDataHeader.
    let common_caps: [u32; 1] = [(1 << proto::COMMON_CAP_PROTOCOL_AUTH_SELECTION)
        | (1 << proto::COMMON_CAP_MINI_HEADER)];

    let mut w = proto::Writer::new();
    w.u32(connection_id);
    w.u8(channel_type);
    w.u8(0); // channel_id
    w.u32(common_caps.len() as u32);
    w.u32(channel_caps.len() as u32);
    w.u32(proto::LINK_CAPS_OFFSET);
    for cap in common_caps.iter() {
        w.u32(*cap);
    }
    for cap in channel_caps.iter() {
        w.u32(*cap);
    }
    w.into_vec()
}

/// Parse a `SpiceLinkReply` body.
pub fn parse_link_reply(body: &[u8]) -> Result<LinkReply, String> {
    const FIXED: usize = 4 + proto::SPICE_TICKET_PUBKEY_BYTES + 12; // 178
    if body.len() < FIXED {
        return Err(format!(
            "SPICE link reply is {} bytes, expected at least {}",
            body.len(),
            FIXED
        ));
    }
    let mut r = Reader::new(body);
    let error = r.u32();
    let pub_key = r.bytes(proto::SPICE_TICKET_PUBKEY_BYTES).to_vec();
    let num_common_caps = r.u32() as usize;
    let num_channel_caps = r.u32() as usize;
    let caps_offset = r.u32() as usize;

    let caps_bytes = 4usize
        .checked_mul(num_common_caps.saturating_add(num_channel_caps))
        .ok_or_else(|| "SPICE link reply has an implausible capability count".to_string())?;
    let caps_end = caps_offset
        .checked_add(caps_bytes)
        .ok_or_else(|| "SPICE link reply capability offset overflows".to_string())?;
    if caps_offset < FIXED || caps_end > body.len() {
        return Err(format!(
            "SPICE link reply capabilities at offset {} ({} words) overrun the {}-byte body",
            caps_offset,
            num_common_caps + num_channel_caps,
            body.len()
        ));
    }

    let mut common_caps = Vec::with_capacity(num_common_caps);
    let mut channel_caps = Vec::with_capacity(num_channel_caps);
    let mut caps = Reader::new(&body[caps_offset..]);
    for _ in 0..num_common_caps {
        common_caps.push(caps.u32());
    }
    for _ in 0..num_channel_caps {
        channel_caps.push(caps.u32());
    }

    Ok(LinkReply {
        error,
        pub_key,
        common_caps,
        channel_caps,
    })
}

/// Read a DER tag-length-value header at `at`; returns (tag, value offset, value length).
fn der_tlv(der: &[u8], at: usize) -> Result<(u8, usize, usize), String> {
    let tag = *der
        .get(at)
        .ok_or_else(|| format!("truncated DER: no tag at offset {}", at))?;
    let mut p = at + 1;
    let first = *der
        .get(p)
        .ok_or_else(|| format!("truncated DER: no length at offset {}", p))?;
    p += 1;
    let len = if first < 0x80 {
        first as usize
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return Err(format!(
                "unsupported DER length encoding 0x{:02x} at offset {}",
                first, at
            ));
        }
        let mut v = 0usize;
        for _ in 0..n {
            let b = *der
                .get(p)
                .ok_or_else(|| format!("truncated DER length at offset {}", p))?;
            v = (v << 8) | b as usize;
            p += 1;
        }
        v
    };
    let end = p
        .checked_add(len)
        .ok_or_else(|| format!("DER length overflow at offset {}", at))?;
    if end > der.len() {
        return Err(format!(
            "DER value at offset {} runs {} bytes past the {}-byte buffer",
            at,
            end - der.len(),
            der.len()
        ));
    }
    Ok((tag, p, len))
}

/// Extract (n, e) from a DER SubjectPublicKeyInfo, as big-endian byte strings.
///
/// Structure: SEQUENCE { SEQUENCE { OID, NULL }, BIT STRING {
/// SEQUENCE { INTEGER n, INTEGER e } } }. This is the parse implemented by
/// `create_rsa_from_mb` in `docs/spice-html5/ticket.js`.
pub fn parse_der_pubkey(der: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let (tag, spki_at, _) = der_tlv(der, 0)?;
    if tag != 0x30 {
        return Err(format!(
            "RSA public key: expected SEQUENCE at offset 0, found tag 0x{:02x}",
            tag
        ));
    }

    let (tag, alg_at, alg_len) = der_tlv(der, spki_at)?;
    if tag != 0x30 {
        return Err(format!(
            "RSA public key: expected AlgorithmIdentifier SEQUENCE, found tag 0x{:02x}",
            tag
        ));
    }

    let (tag, bits_at, bits_len) = der_tlv(der, alg_at + alg_len)?;
    if tag != 0x03 {
        return Err(format!(
            "RSA public key: expected BIT STRING, found tag 0x{:02x}",
            tag
        ));
    }
    let unused = *der
        .get(bits_at)
        .ok_or_else(|| "RSA public key: empty BIT STRING".to_string())?;
    if unused != 0 {
        return Err(format!(
            "RSA public key: BIT STRING has {} unused bits, expected 0",
            unused
        ));
    }

    let inner_at = bits_at + 1;
    let (tag, seq_at, seq_len) = der_tlv(der, inner_at)?;
    if tag != 0x30 {
        return Err(format!(
            "RSA public key: expected SEQUENCE inside the BIT STRING, found tag 0x{:02x}",
            tag
        ));
    }
    if seq_at + seq_len > inner_at + (bits_len - 1) {
        return Err("RSA public key: inner SEQUENCE overruns the BIT STRING".to_string());
    }

    let (tag, n_at, n_len) = der_tlv(der, seq_at)?;
    if tag != 0x02 {
        return Err(format!(
            "RSA public key: expected INTEGER n, found tag 0x{:02x}",
            tag
        ));
    }
    let (tag, e_at, e_len) = der_tlv(der, n_at + n_len)?;
    if tag != 0x02 {
        return Err(format!(
            "RSA public key: expected INTEGER e, found tag 0x{:02x}",
            tag
        ));
    }
    if n_len == 0 || e_len == 0 {
        return Err("RSA public key: empty modulus or exponent".to_string());
    }

    Ok((
        der[n_at..n_at + n_len].to_vec(),
        der[e_at..e_at + e_len].to_vec(),
    ))
}

/// Encrypt `password` + one trailing NUL with RSA-OAEP(SHA-1).
///
/// Returns exactly [`proto::SPICE_TICKET_KEY_BYTES`] bytes, the size of the
/// `encrypted_data` field of `SpiceLinkAuthTicket`.
pub fn oaep_encrypt_ticket(password: &str, n_be: &[u8], e_be: &[u8]) -> Result<Vec<u8>, String> {
    if password.len() > proto::SPICE_MAX_PASSWORD_LENGTH {
        return Err(format!(
            "SPICE ticket password is {} bytes, the maximum is {}",
            password.len(),
            proto::SPICE_MAX_PASSWORD_LENGTH
        ));
    }
    let n = BigUint::from_bytes_be(n_be);
    let e = BigUint::from_bytes_be(e_be);
    let key = RsaPublicKey::new(n, e)
        .map_err(|err| format!("invalid RSA public key from SPICE server: {}", err))?;

    let mut plaintext = Vec::with_capacity(password.len() + 1);
    plaintext.extend_from_slice(password.as_bytes());
    plaintext.push(0);

    let padding = Oaep::new::<Sha1>();
    let mut rng = rand::rngs::OsRng;
    let ciphertext = key
        .encrypt(&mut rng, padding, &plaintext)
        .map_err(|err| format!("cannot encrypt SPICE ticket: {}", err))?;

    if ciphertext.len() != proto::SPICE_TICKET_KEY_BYTES {
        return Err(format!(
            "SPICE ticket ciphertext is {} bytes, expected {}",
            ciphertext.len(),
            proto::SPICE_TICKET_KEY_BYTES
        ));
    }
    Ok(ciphertext)
}

/// Perform the whole link handshake on a fresh connection: send
/// `SpiceLinkHeader` + `SpiceLinkMess`, parse `SpiceLinkReply`, authenticate
/// with a SPICE ticket, and verify the auth result.
///
/// `connection_id` must be 0 for the main channel and the session id reported
/// by `SPICE_MSG_MAIN_INIT` for every other channel.
pub fn link_connect(
    stream: &mut Stream,
    connection_id: u32,
    channel_type: u8,
    channel_caps: &[u32],
    password: &str,
) -> Result<LinkReply, String> {
    let mess = build_link_mess(connection_id, channel_type, channel_caps);
    let header = build_link_header(mess.len() as u32);
    stream
        .write_all(&header)
        .map_err(|e| format!("cannot send SPICE link header: {}", e))?;
    stream
        .write_all(&mess)
        .map_err(|e| format!("cannot send SPICE link message: {}", e))?;
    stream
        .flush()
        .map_err(|e| format!("cannot flush SPICE link handshake: {}", e))?;

    let mut reply_header = [0u8; 16];
    read_exact(stream, &mut reply_header)?;
    let mut hr = Reader::new(&reply_header);
    let magic = hr.u32();
    let major = hr.u32();
    let minor = hr.u32();
    let size = hr.u32() as usize;
    if magic != proto::SPICE_MAGIC {
        return Err(format!(
            "SPICE link reply has magic 0x{:08x}, expected 0x{:08x} (not a SPICE server?)",
            magic,
            proto::SPICE_MAGIC
        ));
    }
    if major != proto::SPICE_VERSION_MAJOR || minor != proto::SPICE_VERSION_MINOR {
        return Err(format!(
            "SPICE server speaks protocol version {}.{}, this client needs {}.{}",
            major,
            minor,
            proto::SPICE_VERSION_MAJOR,
            proto::SPICE_VERSION_MINOR
        ));
    }
    if size == 0 || size > 4096 {
        return Err(format!("SPICE link reply has implausible size {}", size));
    }

    let mut body = vec![0u8; size];
    read_exact(stream, &mut body)?;
    let reply = parse_link_reply(&body)?;
    if reply.error != proto::LINK_ERR_OK {
        return Err(format!(
            "SPICE server refused the {} channel: {}",
            channel_name(channel_type),
            proto::link_error_text(reply.error)
        ));
    }

    let (n, e) = parse_der_pubkey(&reply.pub_key)?;
    let ticket = oaep_encrypt_ticket(password, &n, &e)?;

    let mut auth = Vec::with_capacity(4 + proto::SPICE_TICKET_KEY_BYTES);
    auth.extend_from_slice(&proto::AUTH_MECHANISM_SPICE.to_le_bytes());
    auth.extend_from_slice(&ticket);
    stream
        .write_all(&auth)
        .map_err(|e| format!("cannot send SPICE auth ticket: {}", e))?;
    stream
        .flush()
        .map_err(|e| format!("cannot flush SPICE auth ticket: {}", e))?;

    let mut code_bytes = [0u8; 4];
    read_exact(stream, &mut code_bytes)?;
    let auth_code = u32::from_le_bytes(code_bytes);
    if auth_code != proto::LINK_ERR_OK {
        return Err(auth_error_text(auth_code));
    }

    Ok(reply)
}

fn auth_error_text(auth_code: u32) -> String {
    if auth_code == proto::LINK_ERR_PERMISSION_DENIED {
        "SPICE authentication failed (wrong password?)".to_string()
    } else {
        format!(
            "SPICE authentication failed: {}",
            proto::link_error_text(auth_code)
        )
    }
}

fn channel_name(channel_type: u8) -> &'static str {
    match channel_type {
        proto::CHANNEL_MAIN => "main",
        proto::CHANNEL_DISPLAY => "display",
        proto::CHANNEL_INPUTS => "inputs",
        proto::CHANNEL_CURSOR => "cursor",
        _ => "unknown",
    }
}

fn read_exact(stream: &mut Stream, buf: &mut [u8]) -> Result<(), String> {
    stream.read_exact(buf).map_err(|e| {
        if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut {
            "SPICE link handshake timed out waiting for the server".to_string()
        } else if e.kind() == ErrorKind::UnexpectedEof {
            "SPICE server closed the connection during the link handshake".to_string()
        } else {
            format!("SPICE link read failed: {}", e)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1024-bit RSA SubjectPublicKeyInfo generated with
    /// `openssl genrsa 1024 | openssl rsa -pubout -outform DER`.
    const TEST_SPKI_HEX: &str = concat!(
        "30819f300d06092a864886f70d010101050003818d0030818902818100",
        "c57ae90fd86ffae0baeafd62edeafae71389ea7af91e4138d6645d811be86f1b",
        "a3ea24f8ced16b84f180de43a64aacb86aa30f31e0104729abadac8e1ec95a50",
        "125623ebf7d673787919d4fa7752576e9d81f5e0e981fdc90488be53dc36eac5",
        "0b3486b3e36455f6e2560c49d778be5cecb5a86c003a86b34a866323d4d1f08",
        "10203010001",
    );

    fn decode_hex(s: &str) -> Vec<u8> {
        assert!(s.len() % 2 == 0, "hex string has odd length");
        let mut out = Vec::with_capacity(s.len() / 2);
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let hi = (bytes[i] as char)
                .to_digit(16)
                .unwrap_or_else(|| panic!("bad hex digit at {}", i));
            let lo = (bytes[i + 1] as char)
                .to_digit(16)
                .unwrap_or_else(|| panic!("bad hex digit at {}", i + 1));
            out.push((hi * 16 + lo) as u8);
            i += 2;
        }
        out
    }

    #[test]
    fn link_mess_layout_matches_spiceconn_js() {
        // spiceconn.js send_hdr: magic "REDQ", version 2.2, size = 18 for no
        // channel caps; common_caps = 1 word with bits 0 and 3 set.
        let mess = build_link_mess(7, proto::CHANNEL_INPUTS, &[0xdead_beef]);
        assert_eq!(mess.len(), 18 + 4 + 4);
        assert_eq!(&mess[0..4], &7u32.to_le_bytes());
        assert_eq!(mess[4], proto::CHANNEL_INPUTS);
        assert_eq!(mess[5], 0); // channel_id
        assert_eq!(&mess[6..10], &1u32.to_le_bytes()); // num_common_caps
        assert_eq!(&mess[10..14], &1u32.to_le_bytes()); // num_channel_caps
        assert_eq!(&mess[14..18], &18u32.to_le_bytes()); // caps_offset
        assert_eq!(&mess[18..22], &0b1001u32.to_le_bytes());
        assert_eq!(&mess[22..26], &0xdead_beefu32.to_le_bytes());

        let header = build_link_header(mess.len() as u32);
        assert_eq!(&header[0..4], b"REDQ");
        assert_eq!(&header[4..8], &2u32.to_le_bytes());
        assert_eq!(&header[8..12], &2u32.to_le_bytes());
        assert_eq!(&header[12..16], &26u32.to_le_bytes());
    }

    #[test]
    fn link_mess_main_channel_has_no_channel_caps() {
        let mess = build_link_mess(0, proto::CHANNEL_MAIN, &[]);
        assert_eq!(mess.len(), 18);
        assert_eq!(&mess[14..18], &18u32.to_le_bytes());
    }

    #[test]
    fn parse_link_reply_reads_pubkey_and_caps() {
        let der = decode_hex(TEST_SPKI_HEX);
        assert_eq!(der.len(), proto::SPICE_TICKET_PUBKEY_BYTES);
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes()); // error = OK
        body.extend_from_slice(&der);
        body.extend_from_slice(&1u32.to_le_bytes()); // num_common_caps
        body.extend_from_slice(&1u32.to_le_bytes()); // num_channel_caps
        body.extend_from_slice(&18u32.to_le_bytes()); // caps_offset
        body.extend_from_slice(&0b1001u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());

        let reply = parse_link_reply(&body).expect("reply should parse");
        assert_eq!(reply.error, proto::LINK_ERR_OK);
        assert_eq!(reply.pub_key, der);
        assert_eq!(reply.common_caps, vec![0b1001]);
        assert_eq!(reply.channel_caps, vec![0]);

        assert!(parse_link_reply(&body[..100]).is_err());
        let mut bad = body.clone();
        bad[14..18].copy_from_slice(&(body.len() as u32 + 8).to_le_bytes());
        assert!(parse_link_reply(&bad).is_err());
    }

    #[test]
    fn parse_der_pubkey_extracts_synthetic_1024_bit_key() {
        let der = decode_hex(TEST_SPKI_HEX);
        let (n, e) = parse_der_pubkey(&der).expect("SPKI should parse");
        // INTEGER n keeps its DER leading zero byte (RFC 8017 requires the
        // positive encoding); 128 value bytes + 1 sign byte.
        assert_eq!(n.len(), 129);
        assert_eq!(n[0], 0x00);
        assert_eq!(&n[1..5], &[0xc5, 0x7a, 0xe9, 0x0f]);
        assert_eq!(e, vec![0x01, 0x00, 0x01]);
        assert_eq!(BigUint::from_bytes_be(&n).bits(), 1024);
    }

    #[test]
    fn parse_der_pubkey_rejects_malformed_blobs() {
        // Missing the BIT STRING.
        assert!(parse_der_pubkey(&[0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86]).is_err());
        // A DER length that overruns the buffer.
        assert!(parse_der_pubkey(&[0x30, 0x82, 0x01, 0x00, 0x00]).is_err());
        // Not a sequence at all.
        assert!(parse_der_pubkey(&[0x02, 0x01, 0x01]).is_err());
        assert!(parse_der_pubkey(&[]).is_err());
    }

    #[test]
    fn oaep_ticket_is_128_bytes_for_empty_and_max_password() {
        let der = decode_hex(TEST_SPKI_HEX);
        let (n, e) = parse_der_pubkey(&der).expect("SPKI should parse");

        let empty = oaep_encrypt_ticket("", &n, &e).expect("ticket should encrypt");
        assert_eq!(empty.len(), proto::SPICE_TICKET_KEY_BYTES);

        let password = "p".repeat(proto::SPICE_MAX_PASSWORD_LENGTH);
        let full = oaep_encrypt_ticket(&password, &n, &e).expect("ticket should encrypt");
        assert_eq!(full.len(), proto::SPICE_TICKET_KEY_BYTES);
        // OAEP is randomized: two encryptions of the same ticket differ.
        let again = oaep_encrypt_ticket(&password, &n, &e).expect("ticket should encrypt");
        assert_ne!(full, again);

        let too_long = "p".repeat(proto::SPICE_MAX_PASSWORD_LENGTH + 1);
        assert!(oaep_encrypt_ticket(&too_long, &n, &e).is_err());
    }

    #[test]
    fn auth_error_text_mentions_the_password_for_permission_denied() {
        let text = auth_error_text(proto::LINK_ERR_PERMISSION_DENIED);
        assert!(text.contains("wrong password"), "got: {}", text);
        assert!(auth_error_text(proto::LINK_ERR_CHANNEL_NOT_AVAILABLE)
            .contains("CHANNEL_NOT_AVAILABLE"));
    }
}
