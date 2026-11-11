//! Web Push from this window straight to the phone's push service.
//!
//! Two RFCs and nothing else: the message body is encrypted for the one
//! browser that subscribed (RFC 8291, `aes128gcm`), and the request carries a
//! short ES256 token that says which application server sent it (RFC 8292,
//! VAPID). There is no server of ours in between — Apple's or Google's push
//! service is the only hop, and it sees ciphertext.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use p256::elliptic_curve::sec1::ToSec1Point as _;
use p256::{PublicKey, SecretKey};
use sha2_11::Sha256;

/// One RFC 8188 record holds the whole message; a phone notice is a few
/// hundred bytes and the push services cap the body at 4096 anyway.
const RECORD_SIZE: u32 = 4096;
/// The largest plaintext one record can carry: the record, less the AES-GCM
/// tag and the one-octet padding delimiter.
const MAX_PLAINTEXT: usize = RECORD_SIZE as usize - 16 - 1;
/// How long a VAPID token stands. RFC 8292 §2 caps it at 24 hours; twelve
/// leaves room for a skewed clock on either side.
const VAPID_LIFETIME_SECS: u64 = 12 * 60 * 60;
/// How long the push service may hold a notice for a phone that is off. A
/// question older than this is one the person will see on the window anyway.
pub(crate) const PUSH_TTL_SECS: u32 = 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushError {
    /// The subscription's keys are not a P-256 point and a 16-byte secret.
    BadSubscription,
    /// The message does not fit one record.
    TooLong,
    /// The cipher refused — not expected with keys of the right length.
    Cipher,
}

/// The browser's half of a subscription, decoded.
#[derive(Debug, Clone)]
pub(crate) struct Receiver {
    pub(crate) public: PublicKey,
    pub(crate) public_bytes: [u8; 65],
    pub(crate) auth: [u8; 16],
}

impl Receiver {
    /// From the two base64url strings `PushSubscription.toJSON().keys` hands
    /// over.
    pub(crate) fn from_keys(p256dh: &str, auth: &str) -> Result<Self, PushError> {
        let public_raw = decode(p256dh).ok_or(PushError::BadSubscription)?;
        let auth_raw = decode(auth).ok_or(PushError::BadSubscription)?;
        let public_bytes: [u8; 65] = public_raw
            .as_slice()
            .try_into()
            .map_err(|_| PushError::BadSubscription)?;
        let auth: [u8; 16] = auth_raw
            .as_slice()
            .try_into()
            .map_err(|_| PushError::BadSubscription)?;
        let public =
            PublicKey::from_sec1_bytes(&public_bytes).map_err(|_| PushError::BadSubscription)?;
        Ok(Self {
            public,
            public_bytes,
            auth,
        })
    }
}

/// The uncompressed SEC1 form of a public key — the 65 bytes both RFCs name.
pub(crate) fn public_bytes(key: &PublicKey) -> [u8; 65] {
    let point = key.to_sec1_point(false);
    let mut out = [0u8; 65];
    out.copy_from_slice(point.as_bytes());
    out
}

/// RFC 8291 §3.4: one `aes128gcm` body for one receiver.
///
/// `sender` is a fresh key per message and `salt` fresh random bytes; both are
/// arguments so the RFC's own example can be replayed exactly.
pub(crate) fn encrypt(
    plaintext: &[u8],
    receiver: &Receiver,
    sender: &SecretKey,
    salt: [u8; 16],
) -> Result<Vec<u8>, PushError> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(PushError::TooLong);
    }
    let sender_public = public_bytes(&sender.public_key());
    let shared = sender.diffie_hellman(&receiver.public);
    let (cek, nonce) = derive(
        shared.raw_secret_bytes().as_slice(),
        &receiver.auth,
        &receiver.public_bytes,
        &sender_public,
        &salt,
    );
    let mut record = Vec::with_capacity(plaintext.len() + 1);
    record.extend_from_slice(plaintext);
    // The last (and only) record's padding delimiter.
    record.push(0x02);
    let sealed = seal(&cek, &nonce, &record)?;
    let mut body = Vec::with_capacity(86 + sealed.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(65);
    body.extend_from_slice(&sender_public);
    body.extend_from_slice(&sealed);
    Ok(body)
}

/// The content key and nonce, RFC 8291 §3.3–3.4 and RFC 8188 §2.2.
fn derive(
    ecdh_secret: &[u8],
    auth: &[u8; 16],
    ua_public: &[u8; 65],
    as_public: &[u8; 65],
    salt: &[u8; 16],
) -> ([u8; 16], [u8; 12]) {
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&auth[..]), ecdh_secret)
        .expand(&key_info, &mut ikm)
        .expect("32 bytes is a valid HKDF-SHA-256 length");
    let content = Hkdf::<Sha256>::new(Some(&salt[..]), &ikm);
    let mut cek = [0u8; 16];
    content
        .expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .expect("16 bytes is a valid HKDF-SHA-256 length");
    let mut nonce = [0u8; 12];
    content
        .expand(b"Content-Encoding: nonce\0", &mut nonce)
        .expect("12 bytes is a valid HKDF-SHA-256 length");
    (cek, nonce)
}

fn seal(cek: &[u8; 16], nonce: &[u8; 12], record: &[u8]) -> Result<Vec<u8>, PushError> {
    use aes_gcm::KeyInit as _;
    use aes_gcm::aead::Aead as _;
    let cipher = aes_gcm::Aes128Gcm::new_from_slice(cek).map_err(|_| PushError::Cipher)?;
    let nonce = aes_gcm::Nonce::try_from(&nonce[..]).map_err(|_| PushError::Cipher)?;
    cipher
        .encrypt(&nonce, record)
        .map_err(|_| PushError::Cipher)
}

/// RFC 8292 §2: the `Authorization: vapid t=<jwt>, k=<public key>` value for
/// one push service origin.
pub(crate) fn vapid_authorization(
    signer: &SecretKey,
    endpoint: &str,
    subject: &str,
    now_secs: u64,
) -> Option<String> {
    use p256::ecdsa::signature::Signer as _;
    let audience = audience_of(endpoint)?;
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = serde_json::json!({
        "aud": audience,
        "exp": now_secs + VAPID_LIFETIME_SECS,
        "sub": subject,
    });
    let claims = URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
    let signing_input = format!("{header}.{claims}");
    let key = p256::ecdsa::SigningKey::from(signer);
    let signature: p256::ecdsa::Signature = key.sign(signing_input.as_bytes());
    let token = format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes().as_slice())
    );
    let public = URL_SAFE_NO_PAD.encode(public_bytes(&signer.public_key()));
    Some(format!("vapid t={token}, k={public}"))
}

/// The push services a subscription may name, by host: the one name, or
/// (leading dot) any name under it. The window posts to nothing else — a
/// paired phone cannot make it post to this machine's tailnet name, another
/// Serve site or a LAN address.
const PUSH_SERVICES: &[&str] = &[
    ".push.apple.com",
    "fcm.googleapis.com",
    ".push.services.mozilla.com",
    ".notify.windows.com",
];

fn push_service(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    PUSH_SERVICES.iter().any(|service| {
        if service.starts_with('.') {
            host.len() > service.len() && host.ends_with(service)
        } else {
            host == *service
        }
    })
}

/// The origin of a push endpoint — the JWT's `aud`. Only an `https` endpoint
/// on a known push service's name, on its default port and with no user in
/// it, is a push service; anything else is refused before a request is made.
pub(crate) fn audience_of(endpoint: &str) -> Option<String> {
    let parsed = url::Url::parse(endpoint).ok()?;
    let url::Host::Domain(host) = parsed.host()? else {
        return None;
    };
    let plain = parsed.scheme() == "https"
        && parsed.port().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none();
    if !plain || !push_service(host) {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}

pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let trimmed = text.trim().trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(trimmed.replace('+', "-").replace('/', "_"))
        .ok()
}

pub(crate) fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A fresh P-256 key from the operating system's generator.
pub(crate) fn random_secret_key() -> Option<SecretKey> {
    // A uniformly random 32-byte string is a valid scalar except with
    // probability about 2^-32; a few tries make the failure unreachable.
    for _ in 0..8 {
        let bytes = super::random_bytes::<32>()?;
        if let Ok(key) = SecretKey::from_slice(&bytes) {
            return Some(key);
        }
    }
    None
}

/// What the push service said about one delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    Sent,
    /// 404 or 410: the subscription is gone and must be forgotten.
    Gone,
    /// Anything else — kept, and tried again on the next notice.
    Failed,
}

pub(crate) fn delivery_of(status: u16) -> Delivery {
    match status {
        200..=299 => Delivery::Sent,
        404 | 410 => Delivery::Gone,
        _ => Delivery::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(text: &str) -> Vec<u8> {
        decode(&text.replace(' ', "")).expect("base64url")
    }

    // RFC 8291 Section 5 and Appendix A, verbatim.
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const AS_PUBLIC: &str =
        "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const ECDH_SECRET: &str = "kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs";
    const CEK: &str = "oIhVW04MRdy2XN9CiKLxTg";
    const NONCE: &str = "4h_95klXJ5E_qnoN";
    const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml\
                        mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT\
                        pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    fn sender() -> SecretKey {
        SecretKey::from_slice(&b(AS_PRIVATE)).expect("RFC sender key")
    }

    fn salt() -> [u8; 16] {
        b(SALT).try_into().expect("16 bytes")
    }

    #[test]
    fn rfc8291_appendix_a_intermediate_values_are_reproduced() {
        let receiver = Receiver::from_keys(UA_PUBLIC, AUTH).expect("RFC receiver");
        let sender = sender();
        assert_eq!(public_bytes(&sender.public_key()).to_vec(), b(AS_PUBLIC));
        let shared = sender.diffie_hellman(&receiver.public);
        assert_eq!(
            shared.raw_secret_bytes().as_slice(),
            b(ECDH_SECRET).as_slice()
        );
        let as_public: [u8; 65] = b(AS_PUBLIC).try_into().expect("65 bytes");
        let (cek, nonce) = derive(
            shared.raw_secret_bytes().as_slice(),
            &receiver.auth,
            &receiver.public_bytes,
            &as_public,
            &salt(),
        );
        assert_eq!(cek.to_vec(), b(CEK));
        assert_eq!(nonce.to_vec(), b(NONCE));
    }

    #[test]
    fn rfc8291_section_5_message_is_reproduced_byte_for_byte() {
        let receiver = Receiver::from_keys(UA_PUBLIC, AUTH).expect("RFC receiver");
        let body = encrypt(
            b"When I grow up, I want to be a watermelon",
            &receiver,
            &sender(),
            salt(),
        )
        .expect("encrypts");
        assert_eq!(body.len(), 144, "the RFC's Content-Length");
        assert_eq!(body, b(BODY));
    }

    #[test]
    fn a_receiver_with_wrong_key_lengths_is_refused() {
        assert_eq!(
            Receiver::from_keys(AUTH, AUTH).err(),
            Some(PushError::BadSubscription)
        );
        assert_eq!(
            Receiver::from_keys(UA_PUBLIC, UA_PUBLIC).err(),
            Some(PushError::BadSubscription)
        );
        assert_eq!(
            Receiver::from_keys("not base64 !!", AUTH).err(),
            Some(PushError::BadSubscription)
        );
    }

    #[test]
    fn a_message_longer_than_one_record_is_refused() {
        let receiver = Receiver::from_keys(UA_PUBLIC, AUTH).expect("RFC receiver");
        let long = vec![b'a'; MAX_PLAINTEXT + 1];
        assert_eq!(
            encrypt(&long, &receiver, &sender(), salt()).err(),
            Some(PushError::TooLong)
        );
    }

    #[test]
    fn vapid_token_is_an_es256_jwt_for_the_endpoint_origin_that_verifies() {
        use p256::ecdsa::signature::Verifier as _;
        let signer = sender();
        let value = vapid_authorization(
            &signer,
            "https://web.push.apple.com/send/abc?x=1",
            "mailto:phone@host.example.test",
            1_000,
        )
        .expect("an https endpoint");
        let (token, key) = value
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .expect("vapid t=…, k=…");
        assert_eq!(decode(key).expect("key"), b(AS_PUBLIC));
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&decode(parts[0]).expect("header")).expect("json");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["typ"], "JWT");
        let claims: serde_json::Value =
            serde_json::from_slice(&decode(parts[1]).expect("claims")).expect("json");
        assert_eq!(claims["aud"], "https://web.push.apple.com");
        assert_eq!(claims["exp"], 1_000 + VAPID_LIFETIME_SECS);
        assert_eq!(claims["sub"], "mailto:phone@host.example.test");
        let signature_bytes = decode(parts[2]).expect("signature");
        assert_eq!(signature_bytes.len(), 64, "raw r||s, not DER");
        let signature =
            p256::ecdsa::Signature::from_slice(&signature_bytes).expect("a P-256 signature");
        let signing = p256::ecdsa::SigningKey::from(&signer);
        let verifying = signing.verifying_key();
        verifying
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("the token verifies under the advertised key");
    }

    #[test]
    fn only_https_endpoints_have_an_audience() {
        assert_eq!(
            audience_of("https://web.push.apple.com/QGx").as_deref(),
            Some("https://web.push.apple.com")
        );
        assert_eq!(audience_of("http://web.push.apple.com/x"), None);
        for service in [
            "https://web.push.apple.com/QGuQ",
            "https://fcm.googleapis.com/fcm/send/abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            "https://wns2-par02p.notify.windows.com/w/?token=abc",
        ] {
            assert!(audience_of(service).is_some(), "{service}");
        }
        for elsewhere in [
            "https://host.example.test/api/pair",
            "https://push.apple.com/x",
            "https://web.push.apple.com.example.test/x",
            "https://evilpush.apple.com/x",
            "https://fcm.googleapis.com.example.test/x",
            "https://fcm.googleapis.com:8443/x",
            // Userinfo, spelled with a password a mailbox name cannot end in.
            "https://u:!@fcm.googleapis.com/x",
            "https://192.0.2.7/x",
            "https://[::1]/x",
        ] {
            assert_eq!(audience_of(elsewhere), None, "{elsewhere}");
        }
        assert_eq!(audience_of("not a url"), None);
    }

    #[test]
    fn gone_subscriptions_are_told_apart_from_failures() {
        assert_eq!(delivery_of(201), Delivery::Sent);
        assert_eq!(delivery_of(404), Delivery::Gone);
        assert_eq!(delivery_of(410), Delivery::Gone);
        assert_eq!(delivery_of(429), Delivery::Failed);
        assert_eq!(delivery_of(500), Delivery::Failed);
    }
}
